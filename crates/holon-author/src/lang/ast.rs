//! The syntax tree the lowering reads (docs/LANGUAGE.md): what the TypeScript reader, [`super::ts`],
//! makes of a `.holon.ts` text. Every node carries its span, so an error, a check result or a replay
//! can point at the expression that placed a node. The tree keeps a flow's meaning, not its
//! spelling: the imports and the `flow(…)` settings are headers, a handler's `return`s are one
//! `Respond`, a ternary and an `if` are both `If`, and a `let` given a new value is a new `Bind`.

/// A place in the text: 1-based line and column, the column counted in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// From a start to an exclusive end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn to(self, end: Span) -> Span {
        Span { start: self.start, end: end.end }
    }
}

/// An identifier (or a word standing for one) with its span.
#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

/// A whole text: its headers (the `flow(…)` settings and what it imports), then its items.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct File {
    pub headers: Vec<Header>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub kind: HeaderKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HeaderKind {
    /// `flow("name")`.
    Flow(String),
    /// `flow("name", { port: 8080 })`.
    Port(String),
    /// `keep: { runs: 500, days: 30 }`: how many runs the deployment's log keeps, and for how many days
    /// (the limits `keep_runs` and `keep_run_days`); either may be left out, not both.
    Keep { runs: Option<u64>, days: Option<u64> },
    /// `limits: { run_budget_ms: "60s" }`: a limit of the deploy settings (one of [`crate::LIMIT_NAMES`]),
    /// in its unit (a `_ms` limit's duration in milliseconds, a `_seconds` one's in seconds).
    Limit { name: String, value: u64 },
    /// `browsers: ["https://app.example"]`: the origins whose pages may call the flow.
    Browsers(Vec<String>),
    /// An import the flow can do without, `with { optional: "true" }`.
    Optional(Consent),
    /// `import key from "secret:signing_key"`: a secret the flow reads; someone sets it before the
    /// flow works (an optional import is one it can do without).
    Secret(String),
    /// A key of the flow's state (`import state from "state"`), read as `state.beats` and written
    /// as `state.beats = …`: each key used is one header.
    State(String),
    /// `import notes from "collection:notes"`: used as `notes[k]`.
    Collection(String),
    /// `import inbox from "folder:inbox"`: a folder an admin binds on the host, used as
    /// `inbox[name]`.
    Folder(String),
    /// `import shop from "database:shop"`: a database an admin binds on the host, used as
    /// `shop.query("select …")`, `shop.one(…)` and `shop.exec(…)`, or, a document database,
    /// `notes.find("notes", { … })`, `find_one`, `insert`, `update` and `delete`.
    Database(String),
    /// `import updates from "channel:updates"`: packs the flow sends (`updates.send(v)`) to
    /// whoever subscribes, a route answering with `return updates.subscribe()`.
    Channel(String),
    /// `import { charge } from "flow:billing"`: another deployed flow, by its slug, whose actions
    /// this flow runs, `RunsAction("billing/charge")`. Its call noun is the slug with `-` as `_`.
    Calls(String),
}

/// What a file handler runs on: `on.file.created(inbox, …)`, `on.file.deleted(…)`, and the names
/// it takes, `{ matching: "*.csv" }`.
#[derive(Clone, Debug, PartialEq)]
pub struct FileOn {
    pub folder: String,
    /// For files removed from the folder.
    pub deleted: bool,
    /// The glob a file's name must match.
    pub matching: Option<String>,
}

/// What a flow can do without: a secret by its name (`with { optional: "true" }`).
#[derive(Clone, Debug, PartialEq)]
pub enum Consent {
    Secret(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Holon(HolonDef),
    Js(JsDef),
    Route(RouteDef),
    Every(EveryDef),
    OnFile(OnFileDef),
    Action(ActionDef),
    Stmt(Stmt),
}

/// `export const reset = action((input) => …)`: an action, a run by hand by its name
/// (docs/L4s/triggers-L4.md §2). `input` in it is what the caller sent; what it returns is the
/// answer.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionDef {
    pub name: Name,
    /// The name of what was sent.
    pub input: Name,
    /// `action((input) =>`.
    pub head: Span,
    /// `{ log: "lazy" }` or `"unrecorded"`: how its runs are recorded (strict without one).
    pub log: Option<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `on.file.created(inbox, (f) => …, { matching: "…" })` (or `deleted`): a file handler, a run for
/// each file (docs/L4s/triggers-L4.md §2). `f` in it is the file's handle.
#[derive(Clone, Debug, PartialEq)]
pub struct OnFileDef {
    pub on: FileOn,
    /// The name of the file's handle.
    pub file: Name,
    /// `on.file.created(inbox, (f) =>`.
    pub head: Span,
    /// `{ log: "lazy" }` or `"unrecorded"`: how its runs are recorded (strict without one).
    pub log: Option<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `every("2s", () => …)`: a clock handler, a run every period (docs/L4s/triggers-L4.md §2).
/// `now` in it is the time of the tick.
#[derive(Clone, Debug, PartialEq)]
pub struct EveryDef {
    /// The period in seconds, and as written.
    pub seconds: f64,
    pub text: String,
    /// `every("2s", () =>`.
    pub head: Span,
    /// `{ log: "lazy" }` or `"unrecorded"`: how its runs are recorded (strict without one).
    pub log: Option<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `const name = js(function out(inputs) { body }, { fuel, memory })`: a function whose body is
/// JavaScript, run by the engine module (docs/L4s/js-node-L4.md §2). Called by name like any.
#[derive(Clone, Debug, PartialEq)]
pub struct JsDef {
    pub name: Name,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub fuel: Option<Budget>,
    pub memory: Option<Budget>,
    /// The JavaScript between the braces, its types blanked.
    pub body: String,
    /// The braces' span.
    pub body_span: Span,
    pub span: Span,
}

/// A budget as written: `"50M"`, `"16MB"`, `10000000`.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub value: u64,
    pub text: String,
    pub span: Span,
}

/// `function name(inputs) { body }`: a holon, its outputs named by what it returns.
#[derive(Clone, Debug, PartialEq)]
pub struct HolonDef {
    pub name: Name,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// A function's port: its name, an optional type, and the unit's own name for it when that
/// differs.
#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub name: Name,
    pub ty: Option<TypeExpr>,
    pub unit: Option<String>,
}

/// A port's type, as the lowering names it: `Text`, `List<Text>`, `Record`, …
#[derive(Clone, Debug, PartialEq)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

/// `route.get("/path", (req) => …, { resumes: "name" })`.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteDef {
    pub method: Name,
    pub path: Name,
    pub request: Name,
    pub resumes: Option<Name>,
    /// `{ log: "lazy" }` or `"unrecorded"`: how its runs are recorded (strict without one).
    pub log: Option<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
    /// What the statement's sinks are tied to, instead of the handler's trigger: the condition of
    /// the `if` it was written in.
    pub after: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// `const name = expr`, or a `let` given a value.
    Bind { name: Name, value: Expr },
    /// `state.key = expr`.
    StateWrite { key: Name, value: Expr },
    /// `notes[key] = expr`.
    CollectionWrite { collection: Name, key: Expr, value: Expr },
    /// `delete notes[key]`.
    Delete { collection: Name, key: Expr },
    /// A handler's answer: its `return`s folded into one, the value first, then `status`,
    /// `missing`, `headers` and `stream` named.
    Respond(Vec<Arg>),
    /// An expression on its own: an effect, or a flow's output.
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Number { value: f64, text: String },
    /// `"10s"`, `"5m"`, `"2h"`, `"1d"` where a duration is read: in seconds, and as written.
    Duration { seconds: f64, text: String },
    Text(String),
    /// A template literal, `` `a ${x}` ``.
    Interpolated(Vec<Part>),
    Bool(bool),
    /// A bound name, a function, a leaf, an imported collection, or a word in a param position.
    Name(String),
    /// An imported secret, by the secret's name.
    Secret(String),
    /// `state.key`, read.
    State(String),
    Noun(Noun),
    /// `f(args)`.
    Call { callee: Name, args: Vec<Arg> },
    /// A leaf the reader placed for one of JavaScript's own forms (`Math.sqrt(x)`, `a % b`,
    /// `parseInt(s)`, `x.toFixed(2)`), with its params: never a function of the flow's that
    /// shares its name. `number`: the form is a number's and another type's method
    /// (`toString`), so its first argument must be a value the lowering knows is a number.
    Leaf { leaf: Name, params: Vec<(String, String)>, args: Vec<Arg>, number: bool },
    /// `x.length`, `x.concat(…)`: a method a text and a list both have, done by different
    /// leaves, so the lowering picks them by the type of the node that made `subject`.
    TextOrList { subject: Box<Expr>, method: Name, args: Vec<Arg> },
    /// `receiver.method(args)`: `notes.list(…)`, `api.post(…)`, `stripe.verify(…)`.
    Method { receiver: Name, method: Name, args: Vec<Arg> },
    /// `base.name`: an output or a field, decided by the lowering.
    Field { base: Box<Expr>, name: Name },
    /// `base[key]`: a collection read when `base` is a declared collection, else a lookup.
    Index { base: Box<Expr>, key: Box<Expr> },
    /// `{ ...spread, key: value, … }`.
    Record { spread: Option<Box<Expr>>, fields: Vec<(Key, Expr)> },
    List(Vec<Expr>),
    Not(Box<Expr>),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    /// `cond ? then : els`, and an `if` joined; without `els`, absent when `cond` doesn't hold.
    If { cond: Box<Expr>, then: Box<Expr>, els: Option<Box<Expr>> },
    /// `switch (subject) { case …: return value … default: return els }`. `open` is the brace's
    /// span.
    Match { subject: Box<Expr>, arms: Vec<Arm>, els: Option<Box<Expr>>, open: Span },
    /// `await event("name", { expires: "1h" })`.
    Await { name: Name, expires: Option<Box<Expr>> },
    /// `(params) => body`, written in place for a higher-order leaf: `map`, `fold`, `filter`, …
    Lambda { params: Vec<Name>, body: Box<Expr> },
    /// `( expr )`, kept: parentheses are the author saying "two nodes" where an operator chain
    /// would otherwise flatten.
    Paren(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noun {
    Now,
    Random,
}

/// A record literal's key, as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub text: String,
    pub quoted: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    Expr(Expr),
}

/// A call's argument: positional, or named (an option's field, `{ encoding: "hex" }`).
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<Name>,
    pub value: Expr,
}

/// A `switch`'s case: the values it is chosen by, and its answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub patterns: Vec<Expr>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    /// `value when cond`: the value, present only when `cond` holds (an `if` without an `else`).
    When,
    Coalesce,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    /// As the lowering names it.
    pub fn text(self) -> &'static str {
        match self {
            BinOp::When => "when",
            BinOp::Coalesce => "??",
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }

    /// Binding power, lowest first (docs/LANGUAGE.md §5.6). Every operator is left-associative.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::When => 1,
            BinOp::Coalesce => 2,
            BinOp::Or => 3,
            BinOp::And => 4,
            BinOp::Eq | BinOp::Ne => 5,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 6,
            BinOp::Add | BinOp::Sub => 7,
            BinOp::Mul | BinOp::Div => 8,
        }
    }
}
