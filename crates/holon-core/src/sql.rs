//! What a SQL statement a flow sends to a database reads and writes (docs/L4s/databases-L4.md §4).
//!
//! A database leaf's statement is a frozen param, written as text in the flow, so the tables it
//! touches are known before it runs and its manifest names them: `DbRead("shop/orders")`,
//! `DbWrite("shop/refunds")`. This module finds them, with no dependency (core keeps to its four):
//! a tokenizer and a scan of a deliberately small part of SQL, which **refuses** what it doesn't
//! understand rather than guess. Over-reporting a table only asks for more; missing one would let a
//! flow touch what it wasn't granted, so every doubt is a refusal, with its reason.
//!
//! - **What it takes.** A read is one `select` (or `with … select`); a write is one `insert`,
//!   `update` or `delete` (`replace into` too). Values are parameters, `$1`, `$2`, …, never put
//!   into the text.
//! - **Tables** are what follows `from`, `join`, `update`, `into`, `delete from` and a delete's
//!   `using`, outside a function's parentheses; a name a `with` defines isn't one where it is
//!   visible. `schema.table` keeps its dot. An unquoted name is folded to lower case.
//! - **Refused,** each saying why: several statements; DDL, transactions and other commands;
//!   locking clauses (`for update`, `lock`); `select … into`; a write's `returning`; a `with`
//!   inside a subquery; a table-valued function; a call to a function not known to have no side
//!   effects and to answer the same each time; the time (`current_timestamp`: pass it in); other
//!   placeholders (`?`, `:name`); comments some engines read as code (`#`, `--x`, `/*!`); a
//!   string with a backslash (engines differ on what it escapes).
//! - **Brackets are code,** as Postgres reads them (`array[…]`, `tags[1]`), so a subquery inside
//!   them is scanned like any other; a name is quoted `"…"` (or `` `…` ``), not SQLite's `[…]`.

use std::collections::{BTreeMap, BTreeSet};

/// What a statement touches, by table, and how many parameters it takes (the highest `$n`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Tables {
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    pub params: usize,
}

/// The tables `sql` reads and writes: a read (`select`) unless `write` (`insert`, `update`,
/// `delete`). Refused, with the reason, when it is anything else or uses what this can't follow.
pub fn tables(sql: &str, write: bool) -> Result<Tables, String> {
    let mut tokens = tokenize(sql)?;
    if tokens.last().is_some_and(|t| t.is(";")) {
        tokens.pop();
    }
    if tokens.iter().any(|t| t.is(";")) {
        return Err("it holds more than one statement: a call sends one".into());
    }
    let Some(first) = tokens.first() else { return Err("it is empty".into()) };
    let first = first.word().unwrap_or("");
    let mut scan = Scan { tokens: &tokens, found: Tables::default(), write };
    if write {
        if !matches!(first, "insert" | "update" | "delete" | "replace") {
            return Err(format!("a write is an insert, an update or a delete, not {}", said(&tokens[0])));
        }
        scan.write_head()?;
    } else {
        if !matches!(first, "select" | "with") {
            return Err(format!("a read is a select, not {}", said(&tokens[0])));
        }
        let visible = scan.ctes()?;
        scan.body(visible.0, tokens.len(), &visible.1)?;
    }
    scan.found.params = tokens.iter().filter_map(|t| if let Tok::Param(n) = t.tok { Some(n) } else { None }).max().unwrap_or(0);
    Ok(scan.found)
}

/// `sql` with each `$n` written `?`, and the parameter each `?` takes, in order: what an engine
/// with only `?` (MySQL) is sent. `$1` used twice is two `?`. Strings, quoted names and comments
/// are kept as they are; a statement [`tables`] refuses is refused.
pub fn positional(sql: &str) -> Result<(String, Vec<usize>), String> {
    tokenize(sql)?;
    let s = sql.as_bytes();
    let (mut out, mut order, mut i, mut kept) = (String::with_capacity(sql.len()), Vec::new(), 0, 0);
    while i < s.len() {
        let skip_to = |from: usize, end: &[u8]| s[from..].windows(end.len()).position(|w| w == end).map_or(s.len(), |n| from + n + end.len());
        match s[i] {
            b'\'' => {
                // A string: '' inside it is a quote (tokenize refused anything else).
                let mut j = i + 1;
                while j < s.len() {
                    if s[j] == b'\'' && s.get(j + 1) == Some(&b'\'') {
                        j += 2;
                    } else if s[j] == b'\'' {
                        break;
                    } else {
                        j += 1;
                    }
                }
                i = j + 1;
            }
            b'"' | b'`' => i = skip_to(i + 1, &s[i..=i]),
            b'-' if s.get(i + 1) == Some(&b'-') => i = skip_to(i, b"\n"),
            b'/' if s.get(i + 1) == Some(&b'*') => i = skip_to(i + 2, b"*/"),
            b'$' => {
                let len = s[i + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
                out.push_str(&sql[kept..i]);
                out.push('?');
                order.push(sql[i + 1..i + 1 + len].parse().map_err(|_| "a parameter's number is too big")?);
                i += 1 + len;
                kept = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[kept.min(sql.len())..]);
    Ok((out, order))
}

/// The one table a statement touches, and the columns it pins to one value each, as `column = $n`
/// (or `$n = column`) joined by `and` at the top of its `where`; for an insert, the columns its
/// one row of values gives as `$n`. Column → the parameter's number. None when it touches more
/// than one table, is a union, or has an `or` there: a host claims the row a pinned primary key
/// names, and the whole table otherwise (docs/L4s/databases-L4.md §7.2).
pub fn pins(sql: &str, write: bool) -> Option<(String, BTreeMap<String, usize>)> {
    let found = tables(sql, write).ok()?;
    let mut touched = found.reads.iter().chain(&found.writes).collect::<BTreeSet<_>>().into_iter();
    let (Some(table), None) = (touched.next(), touched.next()) else { return None };
    let tokens = tokenize(sql).ok()?;
    let column = |t: &[Token]| -> Option<String> {
        let name = |t: &Token| match &t.tok {
            Tok::Word(_) => Some(t.lower.clone()),
            Tok::Quoted(q) => Some(q.to_lowercase()),
            _ => None,
        };
        match t {
            [c] => name(c),
            [_, dot, c] if dot.is(".") => name(c),
            _ => None,
        }
    };
    // Each token's depth in parentheses.
    let mut depth = 0usize;
    let depths: Vec<usize> = tokens
        .iter()
        .map(|t| {
            if t.is(")") {
                depth = depth.saturating_sub(1);
            }
            let d = depth;
            if t.is("(") {
                depth += 1;
            }
            d
        })
        .collect();
    let top = |w: &str| tokens.iter().zip(&depths).position(|(t, d)| *d == 0 && t.is_word(w));
    let mut pinned = BTreeMap::new();
    if tokens.first().is_some_and(|t| t.is_word("insert") || t.is_word("replace")) {
        // insert into t (a, b) values ($1, $2): one row, each column from one parameter.
        let values = top("values")?;
        let open = tokens.iter().position(|t| t.is("(")).filter(|&o| o < values);
        let Some(open) = open else { return Some((table.clone(), pinned)) };
        let names: Vec<Option<String>> = tokens[open + 1..].split(|t| t.is(")")).next()?.split(|t| t.is(",")).map(column).collect();
        let close = (values + 2..tokens.len()).find(|&j| tokens[j].is(")") && depths[j] == 0)?;
        if tokens.get(close + 1).is_some_and(|t| t.is(",")) {
            return None;
        }
        let given: Vec<&[Token]> = tokens[values + 2..close].split(|t| t.is(",")).collect();
        if !tokens.get(values + 1)?.is("(") || given.len() != names.len() {
            return None;
        }
        for (name, value) in names.into_iter().zip(given) {
            if let (Some(name), [Token { tok: Tok::Param(n), .. }]) = (name, value) {
                pinned.insert(name, *n);
            }
        }
        return Some((table.clone(), pinned));
    }
    if ["union", "except", "intersect"].iter().any(|w| top(w).is_some()) {
        return None;
    }
    let Some(start) = top("where") else { return Some((table.clone(), pinned)) };
    const ENDS: &[&str] = &["group", "order", "limit", "offset", "having", "window", "returning", "for", "fetch"];
    let end = (start + 1..tokens.len()).find(|&j| depths[j] == 0 && tokens[j].word().is_some_and(|w| ENDS.contains(&w))).unwrap_or(tokens.len());
    // Its conjuncts, split at each top-level `and` that isn't a `between`'s.
    let mut conjuncts: Vec<&[Token]> = Vec::new();
    let (mut from, mut between) = (start + 1, false);
    for j in start + 1..end {
        if depths[j] > 0 {
            continue;
        }
        match tokens[j].word() {
            Some("or") => return None,
            Some("between") => between = true,
            Some("and") if between => between = false,
            Some("and") => {
                conjuncts.push(&tokens[from..j]);
                from = j + 1;
            }
            _ => {}
        }
    }
    conjuncts.push(&tokens[from..end]);
    for c in conjuncts {
        let Some(eq) = c.iter().position(|t| t.is("=")) else { continue };
        let (left, right) = (&c[..eq], &c[eq + 1..]);
        let pin = match (left, right) {
            (l, [Token { tok: Tok::Param(n), .. }]) => column(l).map(|c| (c, *n)),
            ([Token { tok: Tok::Param(n), .. }], r) => column(r).map(|c| (c, *n)),
            _ => None,
        };
        if let Some((c, n)) = pin {
            pinned.insert(c, n);
        }
    }
    Some((table.clone(), pinned))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// A word: a keyword or a name, as written.
    Word(String),
    /// A quoted name (`"x"`, `` `x` ``, `[x]`), exactly.
    Quoted(String),
    Str,
    Num,
    /// `$n`.
    Param(usize),
    Punct(&'static str),
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    /// The word in lower case, for a word.
    lower: String,
}

impl Token {
    fn is(&self, p: &str) -> bool {
        matches!(&self.tok, Tok::Punct(q) if *q == p)
    }
    fn word(&self) -> Option<&str> {
        matches!(self.tok, Tok::Word(_)).then_some(self.lower.as_str())
    }
    fn is_word(&self, w: &str) -> bool {
        self.word() == Some(w)
    }
}

/// A token as an error shows it.
fn said(t: &Token) -> String {
    match &t.tok {
        Tok::Word(w) => format!("\"{w}\""),
        Tok::Quoted(q) => format!("\"{q}\""),
        Tok::Str => "a string".into(),
        Tok::Num => "a number".into(),
        Tok::Param(n) => format!("${n}"),
        Tok::Punct(p) => format!("\"{p}\""),
    }
}

const PUNCT: &[&str] = &["::", "<=", ">=", "<>", "!=", "||", "->>", "->", "(", ")", "[", "]", ",", ".", ";", "*", "=", "<", ">", "+", "-", "/", "%", "&", "|", "~", "^", "!"];

fn tokenize(sql: &str) -> Result<Vec<Token>, String> {
    let s = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let word = |w: &str| Token { tok: Tok::Word(w.to_string()), lower: w.to_ascii_lowercase() };
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if s[i..].starts_with(b"--") {
            // A comment to the end of the line, as every engine reads it only when a space follows.
            if !s.get(i + 2).is_none_or(|c| c.is_ascii_whitespace()) {
                return Err("a -- comment is followed by a space (MySQL reads --x as code)".into());
            }
            while i < s.len() && s[i] != b'\n' {
                i += 1;
            }
        } else if s[i..].starts_with(b"/*") {
            if s.get(i + 2) == Some(&b'!') {
                return Err("a /*! comment is code to MySQL: write it as SQL".into());
            }
            let Some(end) = sql[i + 2..].find("*/") else { return Err("a /* comment isn't closed".into()) };
            if sql[i + 2..i + 2 + end].contains("/*") {
                return Err("a comment inside a comment: engines read that differently".into());
            }
            i += 2 + end + 2;
        } else if c == b'#' {
            return Err("# starts a comment in MySQL and is code elsewhere: write -- instead".into());
        } else if c == b'\'' {
            i += 1;
            loop {
                match s.get(i) {
                    None => return Err("a string isn't closed".into()),
                    Some(b'\\') => return Err("a string has a backslash, which engines read differently: pass it as a parameter ($1)".into()),
                    Some(b'\'') if s.get(i + 1) == Some(&b'\'') => i += 2,
                    Some(b'\'') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            out.push(Token { tok: Tok::Str, lower: String::new() });
        } else if c == b'"' || c == b'`' {
            // A quoted name. Not `[`: Postgres reads brackets as code (an array, a subscript),
            // so what is inside them is scanned as code.
            let close = c;
            let start = i + 1;
            let Some(len) = s[start..].iter().position(|&b| b == close) else { return Err("a quoted name isn't closed".into()) };
            let name = &sql[start..start + len];
            out.push(Token { tok: Tok::Quoted(name.to_string()), lower: name.to_string() });
            i = start + len + 1;
        } else if c == b'$' {
            let start = i + 1;
            let len = s[start..].iter().take_while(|b| b.is_ascii_digit()).count();
            if len == 0 {
                return Err("a $ is a parameter, $1, $2, …: a $ string or a $name isn't supported".into());
            }
            let n: usize = sql[start..start + len].parse().map_err(|_| "a parameter's number is too big")?;
            if n == 0 || n > 999 {
                return Err(format!("${n}: parameters count from $1, up to $999"));
            }
            out.push(Token { tok: Tok::Param(n), lower: String::new() });
            i = start + len;
        } else if c == b'?' {
            return Err("write parameters as $1, $2, …, not ?".into());
        } else if c == b':' && s.get(i + 1) != Some(&b':') {
            return Err("write parameters as $1, $2, …, not :name".into());
        } else if c.is_ascii_digit() || (c == b'.' && s.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'.') {
                i += 1;
            }
            out.push(Token { tok: Tok::Num, lower: String::new() });
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_' || s[i] == b'$') {
                i += 1;
            }
            out.push(word(&sql[start..i]));
        } else if let Some(p) = PUNCT.iter().find(|p| s[i..].starts_with(p.as_bytes())) {
            out.push(Token { tok: Tok::Punct(p), lower: String::new() });
            i += p.len();
        } else {
            let ch = sql[i..].chars().next().unwrap_or('?');
            return Err(format!("it has a {ch:?}, which a statement here doesn't take"));
        }
    }
    Ok(out)
}

/// Words that make a statement something other than a read or a write of rows.
const REFUSED: &[(&str, &str)] = &[
    ("create", "it changes the schema, which is the admin's to do"),
    ("drop", "it changes the schema, which is the admin's to do"),
    ("alter", "it changes the schema, which is the admin's to do"),
    ("truncate", "it empties a table: delete its rows instead"),
    ("rename", "it changes the schema, which is the admin's to do"),
    ("grant", "it changes permissions, which is the admin's to do"),
    ("revoke", "it changes permissions, which is the admin's to do"),
    ("begin", "Holon runs each run's writes in one transaction itself"),
    ("commit", "Holon runs each run's writes in one transaction itself"),
    ("rollback", "Holon runs each run's writes in one transaction itself"),
    ("savepoint", "Holon runs each run's writes in one transaction itself"),
    ("release", "Holon runs each run's writes in one transaction itself"),
    ("transaction", "Holon runs each run's writes in one transaction itself"),
    ("lock", "a lock is the host's to take (its commit locks)"),
    ("attach", "it reaches another database"),
    ("detach", "it reaches another database"),
    ("pragma", "it changes the database's settings"),
    ("vacuum", "it is the admin's to run"),
    ("analyze", "it is the admin's to run"),
    ("copy", "it reads or writes files"),
    ("load", "it loads files or code"),
    ("outfile", "it writes a file"),
    ("dumpfile", "it writes a file"),
    ("call", "it calls a procedure, which may do anything"),
    ("execute", "it runs a statement Holon can't see"),
    ("exec", "it runs a statement Holon can't see"),
    ("prepare", "it runs a statement Holon can't see"),
    ("handler", "it reads a table outside a query"),
    ("listen", "it is a session's command"),
    ("notify", "it sends a message: use an effect"),
    ("current_timestamp", "it changes every time it is asked, so a run can't be checked: pass the time in as a parameter (now)"),
    ("current_date", "it changes every time it is asked, so a run can't be checked: pass the time in as a parameter (now)"),
    ("current_time", "it changes every time it is asked, so a run can't be checked: pass the time in as a parameter (now)"),
    ("localtime", "it changes every time it is asked, so a run can't be checked: pass the time in as a parameter (now)"),
    ("localtimestamp", "it changes every time it is asked, so a run can't be checked: pass the time in as a parameter (now)"),
];

/// Functions known to have no side effects and to answer the same for the same arguments.
const FUNCTIONS: &[&str] = &[
    "abs", "avg", "bool_and", "bool_or", "ceil", "ceiling", "char_length", "character_length", "coalesce", "concat", "concat_ws", "count",
    "date", "datetime", "every", "floor", "greatest", "group_concat", "ifnull", "iif", "instr", "json", "json_array", "json_array_length",
    "json_extract", "json_object", "json_type", "julianday", "least", "length", "like", "lower", "ltrim", "max", "min", "mod", "nullif",
    "octet_length", "position", "power", "printf", "quote", "replace", "reverse", "round", "rtrim", "sign", "sqrt", "strftime", "string_agg",
    "substr", "substring", "sum", "total", "trim", "trunc", "typeof", "unicode", "upper", "array_agg", "json_agg", "jsonb_agg", "row_number",
    "rank", "dense_rank", "lag", "lead", "first_value", "last_value", "ntile", "percent_rank", "cume_dist", "to_char", "date_trunc",
    "date_part", "split_part", "left", "right", "lpad", "rpad", "repeat", "format", "if", "isnull",
];

/// Words that take parentheses without being a function: what follows them is a group, a
/// subquery or a list.
const PAREN_WORDS: &[&str] = &[
    "in", "exists", "values", "over", "filter", "any", "all", "some", "as", "not", "and", "or", "then", "else", "when", "where", "on",
    "using", "by", "from", "join", "select", "set", "having", "case", "is", "between", "like", "within", "partition", "union", "except",
    "intersect", "distinct", "returning", "row", "lateral", "with", "recursive", "materialized", "conflict", "do", "update", "nothing",
    "key", "duplicate", "limit", "offset", "escape", "array", "into",
];

/// Functions whose parentheses hold words of their own (`cast(x as text)`, `extract(year from d)`):
/// a `from` there names no table.
const SPECIAL_CALLS: &[&str] = &["cast", "extract", "trim", "substring", "position", "overlay"];

/// Types a cast names with a size (`varchar(20)`): not calls.
const SIZED_TYPES: &[&str] = &["varchar", "char", "character", "decimal", "numeric", "nvarchar", "nchar", "bit", "varbit", "float", "time", "timestamp", "interval"];

/// Words that end a table's reference (never its alias).
const NOT_ALIAS: &[&str] = &[
    "where", "join", "inner", "left", "right", "full", "outer", "cross", "natural", "on", "using", "group", "order", "limit", "offset",
    "union", "except", "intersect", "set", "values", "select", "returning", "window", "having", "fetch", "lateral", "default", "indexed",
    "not", "for", "as", "from", "with", "do", "by",
];

/// Words that end a from list: a comma after them is no table's.
const ENDS_LIST: &[&str] = &[
    "where", "group", "order", "having", "limit", "offset", "union", "except", "intersect", "window", "returning", "set", "values", "select",
    "fetch", "for", "into", "conflict", "duplicate",
];

struct Scan<'t> {
    tokens: &'t [Token],
    found: Tables,
    write: bool,
}

impl Scan<'_> {
    fn at(&self, i: usize) -> Option<&Token> {
        self.tokens.get(i)
    }

    fn word_at(&self, i: usize) -> Option<&str> {
        self.at(i).and_then(Token::word)
    }

    /// A `with`'s definitions, each scanned with the names before it visible (all of them, for
    /// `with recursive`): where the statement after them starts, and the names it sees.
    fn ctes(&mut self) -> Result<(usize, Vec<String>), String> {
        if !self.at(0).is_some_and(|t| t.is_word("with")) {
            return Ok((0, Vec::new()));
        }
        let mut i = 1;
        let recursive = self.word_at(i) == Some("recursive");
        if recursive {
            i += 1;
        }
        // The names first, so a recursive one sees them all.
        let mut names = Vec::new();
        let mut bodies = Vec::new();
        loop {
            let name = self.name_at(i).ok_or("a with names what it defines")?;
            i += 1;
            if self.at(i).is_some_and(|t| t.is("(")) {
                i = self.close_of(i)? + 1;
            }
            if self.word_at(i) != Some("as") {
                return Err(format!("{name}: a with is name as (select …)"));
            }
            i += 1;
            if self.word_at(i) == Some("not") {
                i += 1;
            }
            if self.word_at(i) == Some("materialized") {
                i += 1;
            }
            if !self.at(i).is_some_and(|t| t.is("(")) {
                return Err(format!("{name}: a with is name as (select …)"));
            }
            let close = self.close_of(i)?;
            names.push(name);
            bodies.push((i + 1, close));
            i = close + 1;
            if self.at(i).is_some_and(|t| t.is(",")) {
                i += 1;
                continue;
            }
            break;
        }
        for (n, (start, end)) in bodies.into_iter().enumerate() {
            let visible: Vec<String> = if recursive { names.clone() } else { names[..n].to_vec() };
            if self.word_at(start).is_none_or(|w| w != "select") {
                return Err(format!("{}: a with defines a select", names[n]));
            }
            self.body(start, end, &visible)?;
        }
        if self.word_at(i).is_none_or(|w| w != "select") {
            return Err("after a with comes the select it is for".into());
        }
        Ok((i, names))
    }

    /// A write's head: the table it writes, then the rest scanned as a body.
    fn write_head(&mut self) -> Result<(), String> {
        let mut i = 1;
        let verb = self.word_at(0).unwrap_or("").to_string();
        // `insert or replace`, `update or ignore` (SQLite).
        if self.word_at(i) == Some("or") {
            i += 2;
        }
        let (table, reads) = match verb.as_str() {
            "insert" | "replace" => {
                if self.word_at(i) == Some("ignore") {
                    i += 1;
                }
                if self.word_at(i) != Some("into") {
                    return Err(format!("an {verb} is {verb} into a table"));
                }
                i += 1;
                let t = self.table_at(&mut i, &[])?;
                if self.at(i).is_some_and(|t| t.is("(")) {
                    i = self.close_of(i)? + 1;
                }
                // An upsert reads the row it may update.
                let upsert = self.tokens[i..].windows(2).any(|w| w[0].is_word("on") && (w[1].is_word("conflict") || w[1].is_word("duplicate")));
                (t, upsert)
            }
            "update" => (self.table_at(&mut i, &[])?, true),
            _ => {
                if self.word_at(i) != Some("from") {
                    return Err("a delete is delete from a table".into());
                }
                i += 1;
                (self.table_at(&mut i, &[])?, true)
            }
        };
        if reads {
            self.found.reads.insert(table.clone());
        }
        self.found.writes.insert(table);
        // Its alias.
        if self.word_at(i) == Some("as") {
            i += 1;
        }
        if self.word_at(i).is_some_and(|w| !NOT_ALIAS.contains(&w)) || matches!(self.at(i).map(|t| &t.tok), Some(Tok::Quoted(_))) {
            i += 1;
        }
        self.body(i, self.tokens.len(), &[])
    }

    /// The name at `i`: a word that isn't a keyword this scan stops at, or a quoted name.
    fn name_at(&self, i: usize) -> Option<String> {
        match &self.at(i)?.tok {
            Tok::Word(_) => Some(self.tokens[i].lower.clone()),
            Tok::Quoted(q) => Some(q.clone()),
            _ => None,
        }
    }

    /// The table named at `i` (`t`, `schema.t`), moving past it: refused when it is a function, or
    /// a name a table can't have here.
    fn table_at(&mut self, i: &mut usize, visible: &[String]) -> Result<String, String> {
        if self.word_at(*i) == Some("only") {
            *i += 1;
        }
        let mut name = self.name_at(*i).ok_or_else(|| format!("expected a table, found {}", self.at(*i).map_or("the end".into(), said)))?;
        *i += 1;
        while self.at(*i).is_some_and(|t| t.is(".")) {
            let part = self.name_at(*i + 1).ok_or("a table's name has nothing after its dot")?;
            name = format!("{name}.{part}");
            *i += 2;
        }
        if self.at(*i).is_some_and(|t| t.is("(")) && !self.write_target(*i) {
            return Err(format!("it reads from a function, {name}(…), which Holon can't follow: read a table"));
        }
        if name.is_empty() || name.len() > 128 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$') {
            return Err(format!("\"{name}\" can't name a table here: letters, digits, _ and a dot"));
        }
        if !name.contains('.') && visible.contains(&name) {
            return Ok(String::new());
        }
        Ok(name)
    }

    /// Whether the `(` at `i` is the column list of the table a write's head names.
    fn write_target(&self, i: usize) -> bool {
        self.write && (1..=4).any(|back| i >= back && self.tokens[i - back].is_word("into"))
    }

    /// The `)` closing the `(` at `i`.
    fn close_of(&self, i: usize) -> Result<usize, String> {
        let mut depth = 0usize;
        for (j, t) in self.tokens.iter().enumerate().skip(i) {
            if t.is("(") {
                depth += 1;
            } else if t.is(")") {
                depth -= 1;
                if depth == 0 {
                    return Ok(j);
                }
            }
        }
        Err("a ( isn't closed".into())
    }

    /// Scan tokens `start..end`, a statement's body, `visible` the names a with defined there.
    fn body(&mut self, start: usize, end: usize, visible: &[String]) -> Result<(), String> {
        // What each open parenthesis is: a call's (a `from` there names no table) or another's;
        // and at each depth, whether a comma there starts another table of a from list.
        let mut calls: Vec<bool> = Vec::new();
        let mut listing: Vec<bool> = vec![false];
        let mut i = start;
        while i < end {
            let t = &self.tokens[i];
            if t.is("(") {
                let prev = i.checked_sub(1).map(|p| &self.tokens[p]);
                let call = prev.and_then(Token::word).is_some_and(|w| SPECIAL_CALLS.contains(&w));
                if self.word_at(i + 1) == Some("with") {
                    return Err("a with inside a subquery isn't supported: put it at the top".into());
                }
                calls.push(call);
                listing.push(false);
                i += 1;
                continue;
            }
            if t.is(")") {
                calls.pop();
                listing.pop();
                if listing.is_empty() {
                    return Err("a ) without its (".into());
                }
                i += 1;
                continue;
            }
            if t.is(",") && listing.last().copied().unwrap_or(false) {
                i += 1;
                self.table_list(&mut i, true, visible)?;
                continue;
            }
            if t.is("::") {
                // A cast to a type: its name, and a size.
                i += 2;
                if self.at(i).is_some_and(|t| t.is("(")) {
                    i = self.close_of(i)? + 1;
                }
                continue;
            }
            let Some(w) = t.word() else {
                i += 1;
                continue;
            };
            if let Some((_, why)) = REFUSED.iter().find(|(r, _)| *r == w) {
                return Err(format!("\"{w}\" isn't taken: {why}"));
            }
            if w == "for" && self.word_at(i + 1).is_some_and(|n| matches!(n, "update" | "share" | "no" | "key")) {
                return Err("a locking clause (for update, for share) isn't taken: Holon checks what a run read when it commits".into());
            }
            if w == "into" && !self.write_target_at(i) {
                return Err("select … into isn't taken: a read answers rows".into());
            }
            if w == "returning" {
                return Err("returning isn't taken: a write answers nothing; read what it wrote in another run".into());
            }
            if self.at(i + 1).is_some_and(|n| n.is("(")) && !PAREN_WORDS.contains(&w) && !SPECIAL_CALLS.contains(&w) && !SIZED_TYPES.contains(&w) && !FUNCTIONS.contains(&w) {
                let before = i.checked_sub(1).and_then(|p| self.word_at(p));
                // A table's column list (`into t (a, b)`) and a with's are no calls.
                if !matches!(before, Some("into" | "table")) {
                    return Err(format!("it calls {w}(…), which Holon can't tell has no side effects and answers the same each time"));
                }
            }
            let in_call = calls.last().copied().unwrap_or(false);
            let distinct_from = w == "from" && i >= 2 && self.word_at(i - 1) == Some("distinct") && matches!(self.word_at(i - 2), Some("is" | "not"));
            let delete_using = w == "using" && self.word_at(0) == Some("delete") && !self.at(i + 1).is_some_and(|n| n.is("("));
            if (w == "from" && !in_call && !distinct_from) || delete_using {
                // A from list: a comma at this depth, past its joins, starts another table.
                if let Some(l) = listing.last_mut() {
                    *l = true;
                }
                i += 1;
                self.table_list(&mut i, true, visible)?;
                continue;
            }
            if w == "join" {
                i += 1;
                self.table_list(&mut i, false, visible)?;
                continue;
            }
            if ENDS_LIST.contains(&w)
                && let Some(l) = listing.last_mut()
            {
                *l = false;
            }
            i += 1;
        }
        Ok(())
    }

    /// Whether the `into` at `i` is a write's (its head), not a select's.
    fn write_target_at(&self, i: usize) -> bool {
        self.write && i <= 3
    }

    /// The tables after a `from` (a list, when `list`) or a `join`: each read, unless a with
    /// defined it. A subquery is left to the body's scan.
    fn table_list(&mut self, i: &mut usize, list: bool, visible: &[String]) -> Result<(), String> {
        loop {
            if self.word_at(*i) == Some("lateral") {
                *i += 1;
            }
            if self.at(*i).is_some_and(|t| t.is("(")) {
                // A subquery, scanned as a body; or tables in parentheses (`(a join b on …)`),
                // scanned as a list and its joins.
                let close = self.close_of(*i)?;
                let mut inner = *i + 1;
                if self.word_at(inner) == Some("with") {
                    return Err("a with inside a subquery isn't supported: put it at the top".into());
                }
                if !matches!(self.word_at(inner), Some("select" | "values")) {
                    self.table_list(&mut inner, true, visible)?;
                }
                self.body(inner, close, visible)?;
                *i = close + 1;
            } else {
                let table = self.table_at(i, visible)?;
                if !table.is_empty() {
                    self.found.reads.insert(table);
                }
            }
            if self.word_at(*i) == Some("as") {
                *i += 1;
            }
            if self.word_at(*i).is_some_and(|w| !NOT_ALIAS.contains(&w)) || matches!(self.at(*i).map(|t| &t.tok), Some(Tok::Quoted(_))) {
                *i += 1;
            }
            if list && self.at(*i).is_some_and(|t| t.is(",")) {
                *i += 1;
                continue;
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn read(sql: &str) -> (BTreeSet<String>, usize) {
        let t = tables(sql, false).unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert!(t.writes.is_empty());
        (t.reads, t.params)
    }

    fn write(sql: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let t = tables(sql, true).unwrap_or_else(|e| panic!("{sql}: {e}"));
        (t.reads, t.writes)
    }

    fn refused(sql: &str, write: bool) -> String {
        tables(sql, write).expect_err(sql)
    }

    #[test]
    fn a_select_reads_every_table_it_names() {
        assert_eq!(read("select id, total from orders where id = $1"), (set(&["orders"]), 1));
        assert_eq!(read("SELECT o.id FROM Orders o JOIN customers AS c ON c.id = o.customer WHERE c.name = $2 AND o.x > $1").0, set(&["orders", "customers"]));
        assert_eq!(read("select * from a, b as bb, public.c where a.x = bb.y").0, set(&["a", "b", "public.c"]));
        assert_eq!(read("select * from a left outer join b using (id) natural join c").0, set(&["a", "b", "c"]));
        assert_eq!(read("select (select max(n) from stock where sku = o.sku) from orders o").0, set(&["orders", "stock"]));
        assert_eq!(read("select * from orders where customer in (select id from customers where vip)").0, set(&["orders", "customers"]));
        assert_eq!(read("select * from (select id from orders) sub").0, set(&["orders"]));
        assert_eq!(read("select a from x union select b from y").0, set(&["x", "y"]));
        assert_eq!(read("select * from (select id from orders) a, customers").0, set(&["orders", "customers"]));
        assert_eq!(read("select * from a join b on a.x = b.y, c where c.z = $1").0, set(&["a", "b", "c"]));
        assert_eq!(read("select * from (a join b on a.x = b.y) left join c on true").0, set(&["a", "b", "c"]));
        assert_eq!(read("select * from t where f in (1, 2), x").0, set(&["t"]), "a comma past where starts no table");
        assert_eq!(read("select * from (values (1), (2)) as v(n)").0, set(&[]));
    }

    /// Postgres reads brackets as code: a subquery in an array or a subscript reads its table.
    #[test]
    fn a_subquery_in_brackets_reads_its_table() {
        assert_eq!(read("select array[(select password from users limit 1)] from orders").0, set(&["orders", "users"]));
        assert_eq!(read("select id from orders where tags[(select count(*) from users)] is not null").0, set(&["orders", "users"]));
        assert_eq!(read("select tags[1], tags[$1] from orders").0, set(&["orders"]));
        assert_eq!(write("insert into orders (tags) values (array[(select password from users limit 1)])"), (set(&["users"]), set(&["orders"])));
        assert!(refused("select * from [orders]", false).contains("expected a table"), "SQLite's [name] isn't a name here");
    }

    #[test]
    fn a_with_defines_names_that_are_not_tables_where_they_are_seen() {
        assert_eq!(read("with big as (select * from orders where total > $1) select * from big join customers c on c.id = big.customer").0, set(&["orders", "customers"]));
        // A non-recursive with sees only the ones before it: a later name is a table there.
        assert_eq!(read("with a as (select * from b), b as (select 1 as x) select * from a").0, set(&["b"]));
        assert_eq!(read("with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select n from t").0, set(&[]));
        assert!(refused("select * from (with x as (select 1) select * from x) y", false).contains("inside a subquery"));
    }

    #[test]
    fn functions_with_from_inside_name_no_table() {
        assert_eq!(read("select extract(year from placed) from orders").0, set(&["orders"]));
        assert_eq!(read("select trim(both 'x' from name), substring(name from 2 for 3) from people").0, set(&["people"]));
        assert_eq!(read("select cast(total as decimal(10, 2)), total::numeric(10,2) from orders").0, set(&["orders"]));
        assert_eq!(read("select * from orders where a is distinct from b").0, set(&["orders"]));
        assert_eq!(read("select count(*) over (partition by c order by d) from t").0, set(&["t"]));
    }

    #[test]
    fn a_write_writes_its_table_and_reads_what_it_matches() {
        assert_eq!(write("insert into refunds (order_id, amount) values ($1, $2)"), (set(&[]), set(&["refunds"])));
        assert_eq!(write("insert into archive select * from orders where id = $1"), (set(&["orders"]), set(&["archive"])));
        assert_eq!(write("update stock set n = n - $1 where sku = $2 and n >= $1"), (set(&["stock"]), set(&["stock"])));
        assert_eq!(write("update orders as o set refunded = true from refunds r where r.order_id = o.id"), (set(&["orders", "refunds"]), set(&["orders"])));
        assert_eq!(write("delete from sessions where expires < $1"), (set(&["sessions"]), set(&["sessions"])));
        assert_eq!(write("delete from a using b where a.id = b.id"), (set(&["a", "b"]), set(&["a"])));
        assert_eq!(write("insert into likes (post, n) values ($1, 1) on conflict (post) do update set n = likes.n + 1"), (set(&["likes"]), set(&["likes"])));
        assert_eq!(write("INSERT OR REPLACE INTO kv (k, v) VALUES ($1, $2)").1, set(&["kv"]));
        assert_eq!(write("replace into kv (k, v) values ($1, $2)").1, set(&["kv"]));
    }

    #[test]
    fn what_isnt_a_read_or_a_write_of_rows_is_refused_saying_why() {
        assert!(refused("delete from t", false).contains("a read is a select"));
        assert!(refused("select 1", true).contains("a write is"));
        assert!(refused("select 1; select 2", false).contains("more than one"));
        assert!(refused("select * from t; drop table t", false).contains("more than one"));
        assert!(refused("select * from t for update", false).contains("locking"));
        assert!(refused("select * into copy from t", false).contains("into"));
        assert!(refused("insert into t (a) values ($1) returning id", true).contains("returning"));
        assert!(refused("select * from generate_series(1, 10)", false).contains("function"));
        assert!(refused("select pg_sleep(10) from t", false).contains("pg_sleep"));
        assert!(refused("select random() from t", false).contains("random"));
        assert!(refused("select * from t where at < current_timestamp", false).contains("parameter"));
        assert!(refused("select * from t where id = ?", false).contains("$1"));
        assert!(refused("select * from t where id = :id", false).contains("$1"));
        assert!(refused("select * from t # x", false).contains("#"));
        assert!(refused("select 5 --1 from secret", false).contains("--"));
        assert!(refused("select /*! from secret */ 1", false).contains("/*!"));
        assert!(refused("select 'it\\'s' from t", false).contains("backslash"));
        assert!(refused("select $$x$$", false).contains("$"));
        assert!(refused("select * from \"Mixed Case\"", false).contains("can't name a table"));
        assert!(refused("", false).contains("empty"));
        assert!(refused("pragma table_info(t)", false).contains("a read is a select"));
        assert!(refused("with x as (select 1) delete from t", false).contains("after a with"));
    }

    #[test]
    fn comments_and_strings_hide_nothing() {
        assert_eq!(read("select 'from secret' as s from t -- from other\n").0, set(&["t"]));
        assert_eq!(read("select /* join secret */ x from t;").0, set(&["t"]));
        assert_eq!(read("select 'it''s' from t").0, set(&["t"]));
    }

    #[test]
    fn a_where_pins_the_columns_it_sets_equal_to_parameters() {
        let pins = |sql: &str, write: bool| pins(sql, write).map(|(t, p)| (t, p.into_iter().collect::<Vec<_>>()));
        let pinned = |t: &str, p: &[(&str, usize)]| Some((t.to_string(), p.iter().map(|(c, n)| (c.to_string(), *n)).collect::<Vec<_>>()));
        assert_eq!(pins("select id, total from orders where id = $1 and refunded = 0", false), pinned("orders", &[("id", 1)]));
        assert_eq!(pins("select * from orders o where $2 = o.id and o.shop = $1 order by id limit 1", false), pinned("orders", &[("id", 2), ("shop", 1)]));
        assert_eq!(pins("update orders set refunded = 1 where id = $1 and refunded = 0", true), pinned("orders", &[("id", 1)]));
        assert_eq!(pins("delete from orders where \"Id\" = $3", true), pinned("orders", &[("id", 3)]));
        assert_eq!(pins("select * from t where at between $1 and $2 and id = $3", false), pinned("t", &[("id", 3)]));
        assert_eq!(pins("select * from t where (a = $1 or b = $2) and id = $3", false), pinned("t", &[("id", 3)]), "a group is one more condition");
        assert_eq!(pins("select * from t where id > $1", false), pinned("t", &[]), "a range pins nothing");
        assert_eq!(pins("select count(*) from t", false), pinned("t", &[]));
        assert_eq!(pins("insert into refunds (order_id, amount, note) values ($1, $2, 'x')", true), pinned("refunds", &[("amount", 2), ("order_id", 1)]));
        assert_eq!(pins("insert into t values ($1, $2)", true), pinned("t", &[]), "no column list: nothing known");
        assert_eq!(pins("insert into t (a) values ($1), ($2)", true), None, "two rows");
        assert_eq!(pins("select * from t where id = $1 or id = $2", false), None);
        assert_eq!(pins("select * from a join b on a.id = b.id where a.id = $1", false), None, "two tables");
        assert_eq!(pins("select * from t where id = $1 union select * from t where id = $2", false), None);
    }

    #[test]
    fn parameters_are_counted_by_the_highest() {
        assert_eq!(read("select * from t where a = $2 and b = $1 or c = $2").1, 2);
        assert_eq!(read("select * from t").1, 0);
        assert!(refused("select * from t where a = $0", false).contains("$1"));
    }

    /// MySQL takes `?`: each `$n` is one, in order, wherever it is code, and nowhere else.
    #[test]
    fn dollar_parameters_become_question_marks_in_order() {
        let p = |sql: &str| positional(sql).unwrap();
        assert_eq!(p("update stock set n = n - $1 where sku = $2 and n >= $1"), ("update stock set n = n - ? where sku = ? and n >= ?".to_string(), vec![1, 2, 1]));
        assert_eq!(p("select '$1', \"$2\" from t where a = $3 -- $4\n and b = $1 /* $5 */"), ("select '$1', \"$2\" from t where a = ? -- $4\n and b = ? /* $5 */".to_string(), vec![3, 1]));
        assert_eq!(p("select 'it''s $1' from t where a = $2"), ("select 'it''s $1' from t where a = ?".to_string(), vec![2]));
        assert!(positional("select ? from t").is_err());
    }
}
