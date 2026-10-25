//! The syntax tree of a `.holon` text (docs/LANGUAGE.md): what the parser produces and the
//! lowering reads. Every node carries its span, so an error, a check result or a replay can point
//! at the expression that placed a node. Both forms of the text are here: the expression form
//! (bindings, calls, nouns, routes) and the wiring form (`node`, `a.out -> b.in`), since one file
//! may hold both and the lowering treats them alike.

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

/// A whole text: its headers (deploy settings and declarations), then its items.
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
    Flow(String),
    Port(String),
    /// `every 10s`: the interval, in seconds, and as written.
    Every { seconds: f64, text: String },
    /// The wiring form's `interval 10`.
    Interval(String),
    Browsers(Vec<String>),
    Optional(Consent),
    /// `collection notes`: a name the body may use as `notes[k]`.
    Collection(String),
    /// `folder inbox`: a folder an admin binds on the host, used as `inbox[name]`.
    Folder(String),
    /// `database shop`: a database an admin binds on the host, used as `shop.query("select …")`,
    /// `shop.one(…)` and `shop.exec(…)`.
    Database(String),
    /// `on file inbox`: a run for each file new or changed in the folder (or removed from it,
    /// `deleted`; only names matching a glob, `matching "*.csv"`), which it declares too; the run
    /// reads the file's handle as `ingress`. A flow may have several.
    OnFile(FileOn),
}

/// One `on file` line.
#[derive(Clone, Debug, PartialEq)]
pub struct FileOn {
    pub folder: String,
    /// For files removed from the folder.
    pub deleted: bool,
    /// The glob a file's name must match.
    pub matching: Option<String>,
}

/// What `optional` names: a secret by its name, or a consent name as the server prints one.
#[derive(Clone, Debug, PartialEq)]
pub enum Consent {
    Secret(String),
    Named { name: String, arg: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Holon(HolonDef),
    Js(JsDef),
    Route(RouteDef),
    Stmt(Stmt),
}

/// `holon name(inputs) -> outputs js [fuel N] [memory N] { body }`: a holon whose body is
/// JavaScript, run by the engine module (docs/L4s/js-node-L4.md §2). Placed by name like any.
#[derive(Clone, Debug, PartialEq)]
pub struct JsDef {
    pub name: Name,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub fuel: Option<Budget>,
    pub memory: Option<Budget>,
    /// The JavaScript between the braces, verbatim.
    pub body: String,
    /// The braces' span.
    pub body_span: Span,
    pub span: Span,
}

/// A budget as written: `50M`, `16MB`, `10000000`.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub value: u64,
    pub text: String,
    pub span: Span,
}

/// `holon name(inputs) -> outputs { body }`, in either form.
#[derive(Clone, Debug, PartialEq)]
pub struct HolonDef {
    pub name: Name,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// A holon's port: its name, an optional type, and, in quotes, the unit's own name for it when
/// that differs (how a canvas-grouped holon's proposed names round-trip).
#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub name: Name,
    pub ty: Option<TypeExpr>,
    pub unit: Option<String>,
}

/// `Text`, `List<Text>`, `Record`, …
#[derive(Clone, Debug, PartialEq)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

/// `route METHOD /path (req) [resumes name] { body }`.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteDef {
    pub method: Name,
    pub path: Name,
    pub request: Name,
    pub resumes: Option<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// `name = expr`.
    Bind { name: Name, value: Expr },
    /// `state.key = expr`.
    StateWrite { key: Name, value: Expr },
    /// `notes[key] = expr`.
    CollectionWrite { collection: Name, key: Expr, value: Expr },
    /// `delete notes[key]`.
    Delete { collection: Name, key: Expr },
    /// `respond args…` (the args as a call's, without parentheses; may be empty).
    Respond(Vec<Arg>),
    /// An expression on its own: an effect, or a flow's output.
    Expr(Expr),
    /// The wiring form's `node name = of[arity] { params } at x y`.
    Node { name: Name, of: Name, arity: Option<usize>, params: Vec<(Name, String)>, at: Option<(f64, f64)> },
    /// The wiring form's `a.out -> b.in`.
    Wire { from: (Name, Name), to: (Name, Name) },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Number { value: f64, text: String },
    /// `10s`, `5m`, `2h`, `1d`: in seconds, and as written.
    Duration { seconds: f64, text: String },
    Text(String),
    /// A string with `{expr}` parts.
    Interpolated(Vec<Part>),
    Bool(bool),
    /// A bound name, a holon, a leaf, a declared collection, or a bare word in a param position.
    Name(String),
    /// `secret.name`.
    Secret(String),
    /// `state.key`, read.
    State(String),
    Noun(Noun),
    /// `f(args)`.
    Call { callee: Name, args: Vec<Arg> },
    /// `receiver.method(args)`: `notes.list(…)`, `http.post(…)`, `stripe.verify(…)`, `state.set(…)`.
    Method { receiver: Name, method: Name, args: Vec<Arg> },
    /// `base.name`: an output or a field, decided by the lowering.
    Field { base: Box<Expr>, name: Name },
    /// `base[key]`: a collection read when `base` is a declared collection, else a lookup.
    Index { base: Box<Expr>, key: Box<Expr> },
    /// `{ ..spread, key: value, … }`.
    Record { spread: Option<Box<Expr>>, fields: Vec<(Key, Expr)> },
    List(Vec<Expr>),
    Not(Box<Expr>),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    /// `if cond then then [else els]`.
    If { cond: Box<Expr>, then: Box<Expr>, els: Option<Box<Expr>> },
    /// `match subject { patterns -> value … [else -> els] }`. `open` is the brace's span.
    Match { subject: Box<Expr>, arms: Vec<Arm>, els: Option<Box<Expr>>, open: Span },
    /// `await name[, expires: expr]`.
    Await { name: Name, expires: Option<Box<Expr>> },
    /// `fn(params) -> body`.
    Lambda { params: Vec<Name>, body: Box<Expr> },
    /// `( expr )`, kept: parentheses are the author saying "two nodes" where an operator chain
    /// would otherwise flatten.
    Paren(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noun {
    Now,
    Random,
    Ingress,
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

/// A call's argument: positional, or named (`after: x`, `encoding: hex`).
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<Name>,
    pub value: Expr,
}

/// A `match` arm: the patterns it is chosen by, and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub patterns: Vec<Expr>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
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
    /// As written.
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
