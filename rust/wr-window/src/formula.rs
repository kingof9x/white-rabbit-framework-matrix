//! Formulas typed on the Math page, like `sin(x) * x^2` (`compile_formula` in `math_sims.py`).
//!
//! Only numbers, x, t, pi, e, + - * / % // ^ ** and a list of maths functions are allowed. Anything
//! that can't be worked out gives NaN (log of a negative number) or infinity (1/0); the graph leaves both out.
//!
//! # How it works
//!
//! The Python app checked the formula and then let Python evaluate it. Here it is done by hand, in
//! three steps, so nothing typed can ever run as code:
//!
//! 1. `tokens` cuts the text into tokens: numbers, names, operators, brackets and commas. Any other
//!    character is refused straight away with a message for the user.
//! 2. `Parser` is a recursive-descent parser: one method per precedence level, each calling the next
//!    tighter one. From loosest to tightest:
//!    `expr` (+ -), `term` (* / % //), `unary` (leading - or +), `power` (** or ^, right to left),
//!    `atom` (a number, a bracketed formula, x / t / pi / e, or a function call).
//!    It builds a small tree of `Node`s, and refuses unknown names, unknown functions and the wrong
//!    number of arguments with a message saying what to type instead.
//! 3. `Formula::at(x, t)` walks the tree and works out the value. It never fails: maths with no
//!    answer comes out as NaN (or infinity), which the graph code skips.
//!
//! The precedence follows Python's: `-x**2` is `-(x**2)`, `2**3**2` is `2**9`, and `%` and `//` round
//! towards minus infinity. A formula is parsed once, when the user presses Show; while the graph
//! plays it is worked out hundreds of times per picture (`maths::draw_graph` takes 545 samples), so
//! only the quick tree walk is repeated.

/// Why a formula was refused, as a sentence to show the user under the formula box.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaError(pub String);

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A two-sided operator in the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    /// `%`, Python's modulo (the result has the sign of the right side)
    Mod,
    /// `//`, division rounded down
    FloorDiv,
    /// `**` or `^` (both mean "to the power of")
    Pow,
}

/// One node of the parsed formula's tree.
#[derive(Debug, Clone)]
enum Node {
    /// A number, including pi and e
    Num(f64),
    /// The variable x (the graph's input; on the panel it runs from top to bottom)
    X,
    /// The variable t (time in seconds, for formulas that move)
    T,
    /// Minus the value below
    Neg(Box<Node>),
    /// Left side, operator, right side
    Bin(Op, Box<Node>, Box<Node>),
    /// A function from the allowed lists, with its arguments (already checked: 1 or 2 of them)
    Call(&'static str, Vec<Node>),
}

/// The functions that take one number. "log" and "ln" both mean the natural logarithm.
const ONE_ARGUMENT: [&str; 21] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "exp", "log", "ln", "log10", "sqrt", "abs",
    "floor", "ceil", "round", "sign", "sinc", "trunc",
];
/// The functions that take two numbers.
const TWO_ARGUMENTS: [&str; 2] = ["min", "max"];

/// One token of the typed text.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    /// A word: a variable, a constant or a function name (checked later, by the parser)
    Name(String),
    /// An operator; `^` is stored as "**" since they mean the same
    Op(&'static str),
    Open,
    Close,
    Comma,
}

/// The general "something is wrong" error, for brackets that don't match and similar.
fn typo() -> FormulaError {
    FormulaError("That formula has a typo in it.".into())
}

/// Cuts the text into tokens. Numbers may have a decimal point and an exponent (`1e3`, `.5`);
/// names start with a letter or `_`. Err for any character that isn't allowed, a malformed number
/// (`1.2.3`) or a name stuck to a number (`2x`).
fn tokens(text: &str) -> Result<Vec<Tok>, FormulaError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        // A number starts with a digit, or with a point followed by a digit (".5"); a lone "." is refused
        // below as a character that isn't allowed.
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            // Take digits and points; a number with two points fails to parse just below.
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // An exponent like 1e3 or 2.5E-2
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                // Only a real exponent is taken: "2e" or "2e+" without digits leaves the "e" for the name
                // check below.
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let s: String = chars[start..i].iter().collect();
            out.push(Tok::Num(s.parse().map_err(|_| typo())?));
            // Python refuses a name straight after a number, like 2x
            if i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '_') {
                return Err(typo());
            }
        // A name: letters, digits and underscores, starting with a letter or underscore.
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
        } else {
            // Look at two characters so "**" and "//" are taken as one operator before "*" and "/" alone.
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            let (tok, len) = match (c, two.as_str()) {
                (_, "**") => (Tok::Op("**"), 2),
                (_, "//") => (Tok::Op("//"), 2),
                ('^', _) => (Tok::Op("**"), 1),
                ('+', _) => (Tok::Op("+"), 1),
                ('-', _) => (Tok::Op("-"), 1),
                ('*', _) => (Tok::Op("*"), 1),
                ('/', _) => (Tok::Op("/"), 1),
                ('%', _) => (Tok::Op("%"), 1),
                ('(', _) => (Tok::Open, 1),
                (')', _) => (Tok::Close, 1),
                (',', _) => (Tok::Comma, 1),
                _ => return Err(FormulaError("Only numbers, x, t and maths can be used in a formula.".into())),
            };
            out.push(tok);
            i += len;
        }
    }
    Ok(out)
}

/// Walks the tokens from left to right, building the tree.
struct Parser {
    /// All the tokens of the formula
    toks: Vec<Tok>,
    /// Index of the next token to read
    at: usize,
}

impl Parser {
    /// The next token without taking it; None at the end.
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    /// Takes the next token if it is one of these operators, and returns which one.
    fn eat_op(&mut self, ops: &[&str]) -> Option<&'static str> {
        if let Some(Tok::Op(o)) = self.peek() {
            if ops.contains(o) {
                let o = *o;
                self.at += 1;
                return Some(o);
            }
        }
        None
    }

    /// A whole formula: terms joined by + and -, left to right (so 1 - 2 - 3 is (1 - 2) - 3).
    fn expr(&mut self) -> Result<Node, FormulaError> {
        // Loop rather than recurse, so a chain of + and - groups from the left.
        let mut left = self.term()?;
        while let Some(o) = self.eat_op(&["+", "-"]) {
            let right = self.term()?;
            left = Node::Bin(if o == "+" { Op::Add } else { Op::Sub }, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// Parts joined by *, /, % and //, left to right.
    fn term(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.unary()?;
        while let Some(o) = self.eat_op(&["*", "/", "%", "//"]) {
            let right = self.unary()?;
            let op = match o {
                "*" => Op::Mul,
                "/" => Op::Div,
                "%" => Op::Mod,
                _ => Op::FloorDiv,
            };
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// Any number of leading signs, then a power. Unary minus binds looser than **, as in Python.
    fn unary(&mut self) -> Result<Node, FormulaError> {
        // A leading "+" changes nothing and is just dropped from the tree.
        if let Some(o) = self.eat_op(&["-", "+"]) {
            let inner = self.unary()?;
            return Ok(if o == "-" { Node::Neg(Box::new(inner)) } else { inner });
        }
        self.power()
    }

    /// An atom, optionally raised to a power.
    fn power(&mut self) -> Result<Node, FormulaError> {
        let base = self.atom()?;
        if self.eat_op(&["**"]).is_some() {
            // Right to left, and -x^2 is -(x^2), as in Python
            // The exponent is parsed with `unary`, which itself reaches `power` again: that recursion makes
            // 2**3**2 group as 2**(3**2), and allows a signed exponent like 2**-1.
            let exponent = self.unary()?;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    /// The smallest piece: a number, a bracketed formula, a name, or a function call with its arguments.
    fn atom(&mut self) -> Result<Node, FormulaError> {
        // The formula ended where a value was expected (like "x +").
        let tok = self.peek().cloned().ok_or_else(typo)?;
        self.at += 1;
        match tok {
            Tok::Num(n) => Ok(Node::Num(n)),
            // A bracket starts a whole formula again (lowest precedence), and must be closed.
            Tok::Open => {
                let inner = self.expr()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err(typo());
                }
                self.at += 1;
                Ok(inner)
            }
            Tok::Name(name) => {
                // A name followed by "(" is a function call.
                if self.peek() == Some(&Tok::Open) {
                    self.at += 1;
                    // Only names in the two lists can be called; this is what stops "import os" or "open(...)".
                    let func =
                        ONE_ARGUMENT.iter().chain(TWO_ARGUMENTS.iter()).find(|f| **f == name).copied().ok_or_else(
                            || FormulaError("Only functions like sin(x) or sqrt(x) can be called.".into()),
                        )?;
                    let mut args = Vec::new();
                    // Arguments: formulas separated by commas, until ")". An empty "()" has no arguments,
                    // which the count check below refuses.
                    if self.peek() != Some(&Tok::Close) {
                        loop {
                            args.push(self.expr()?);
                            if self.peek() == Some(&Tok::Comma) {
                                self.at += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    if self.peek() != Some(&Tok::Close) {
                        return Err(typo());
                    }
                    self.at += 1;
                    // Arguments were collected first and are counted here, so the message can say how many
                    // are needed.
                    let wanted = if TWO_ARGUMENTS.contains(&func) { 2 } else { 1 };
                    if args.len() != wanted {
                        return Err(FormulaError(if wanted == 2 {
                            format!("{func}() takes 2 numbers, like {func}(x, 1).")
                        } else {
                            format!("{func}() takes 1 number, like {func}(x).")
                        }));
                    }
                    return Ok(Node::Call(func, args));
                }
                // A plain name: only the two variables and two constants exist.
                match name.as_str() {
                    "x" => Ok(Node::X),
                    "t" => Ok(Node::T),
                    "pi" => Ok(Node::Num(std::f64::consts::PI)),
                    "e" => Ok(Node::Num(std::f64::consts::E)),
                    _ => Err(FormulaError(format!("I don't know '{name}'. Use x, t, pi, e and functions like sin."))),
                }
            }
            // An operator, ")" or "," where a value should be.
            _ => Err(typo()),
        }
    }
}

/// A formula ready to work out.
#[derive(Debug, Clone)]
pub struct Formula {
    /// The parsed tree
    root: Node,
    /// True if the formula has t in it, so the Math page knows it must keep redrawing (it moves)
    pub uses_t: bool,
}

/// Whether t appears anywhere in this tree.
fn uses_t(n: &Node) -> bool {
    match n {
        Node::T => true,
        Node::Neg(a) => uses_t(a),
        Node::Bin(_, a, b) => uses_t(a) || uses_t(b),
        Node::Call(_, args) => args.iter().any(uses_t),
        _ => false,
    }
}

///
/// Rust's `%` keeps the sign of the left side (-7 % 3 = -1); Python's takes the sign of the right
/// side (-7 % 3 = 2). Adding `b` once fixes the result when the signs differ. Modulo by zero is NaN.
/// Python's floored modulo.
fn py_mod(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return f64::NAN;
    }
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) {
        r + b
    } else {
        r
    }
}

/// Works out the value of a tree for this x and t. Division by zero gives infinity (or NaN for 0/0)
/// as usual for floating point; logarithms and square roots of numbers out of range give NaN.
fn eval(n: &Node, x: f64, t: f64) -> f64 {
    match n {
        Node::Num(v) => *v,
        Node::X => x,
        Node::T => t,
        Node::Neg(a) => -eval(a, x, t),
        // Both sides are always worked out (no short cuts), then combined with the operator.
        Node::Bin(op, a, b) => {
            let (a, b) = (eval(a, x, t), eval(b, x, t));
            match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                Op::Mod => py_mod(a, b),
                Op::FloorDiv => (a / b).floor(),
                Op::Pow => a.powf(b),
            }
        }
        Node::Call(f, args) => {
            // Every call has at least one argument (the parser made sure), so args[0] is always there.
            let a = eval(&args[0], x, t);
            match *f {
                "sin" => a.sin(),
                "cos" => a.cos(),
                "tan" => a.tan(),
                "asin" => a.asin(),
                "acos" => a.acos(),
                "atan" => a.atan(),
                "sinh" => a.sinh(),
                "cosh" => a.cosh(),
                "tanh" => a.tanh(),
                "exp" => a.exp(),
                // Python's math.log refuses 0 and negatives; NaN keeps that point off the graph.
                "log" | "ln" => {
                    if a > 0.0 {
                        a.ln()
                    } else {
                        f64::NAN
                    }
                }
                "log10" => {
                    if a > 0.0 {
                        a.log10()
                    } else {
                        f64::NAN
                    }
                }
                "sqrt" => {
                    if a >= 0.0 {
                        a.sqrt()
                    } else {
                        f64::NAN
                    }
                }
                "abs" => a.abs(),
                "floor" => a.floor(),
                "ceil" => a.ceil(),
                // Python's round() sends halves to the even neighbour (round(2.5) = 2), so this does too.
                "round" => a.round_ties_even(),
                "trunc" => a.trunc(),
                // Zero (and NaN) comes back unchanged.
                "sign" => {
                    if a > 0.0 {
                        1.0
                    } else if a < 0.0 {
                        -1.0
                    } else {
                        a
                    }
                }
                // sin(a)/a is 0/0 at a = 0; its limit there is 1.
                "sinc" => {
                    if a == 0.0 {
                        1.0
                    } else {
                        a.sin() / a
                    }
                }
                // Rust's min/max return the other number when one is NaN (Python's would depend on the order).
                "min" => a.min(eval(&args[1], x, t)),
                "max" => a.max(eval(&args[1], x, t)),
                // Can't happen: the parser only builds calls to listed functions.
                _ => f64::NAN,
            }
        }
    }
}

impl Formula {
    ///
    /// Refuses an empty formula, one over 200 characters (also keeps the parser's recursion shallow),
    /// and anything left over after a complete formula (like "x)").
    /// Reads text like 'sin(x) * x^2'.
    pub fn parse(text: &str) -> Result<Formula, FormulaError> {
        let source = text.trim();
        if source.is_empty() {
            return Err(FormulaError("Type a formula, for example sin(x).".into()));
        }
        if source.chars().count() > 200 {
            return Err(FormulaError("That formula is too long.".into()));
        }
        // Cut into tokens (refusing bad characters), then parse a whole formula from the first token.
        let mut p = Parser { toks: tokens(source)?, at: 0 };
        let root = p.expr()?;
        if p.at != p.toks.len() {
            return Err(typo());
        }
        let uses_t = uses_t(&root);
        Ok(Formula { root, uses_t })
    }

    /// The formula's value at this x and time t (seconds). NaN or infinity where there is no answer.
    pub fn at(&self, x: f64, t: f64) -> f64 {
        eval(&self.root, x, t)
    }
}

/// Tests for reading and working out formulas.
#[cfg(test)]
mod tests {
    use super::*;

    /// The value of a formula at x, with t = 0.
    fn y(text: &str, x: f64) -> f64 {
        Formula::parse(text).unwrap().at(x, 0.0)
    }

    /// Operators, precedence, functions and constants give the same numbers Python would.
    #[test]
    fn works_out_like_python() {
        assert_eq!(y("x^2", 3.0), 9.0);
        assert_eq!(y("-x**2", 3.0), -9.0);
        assert_eq!(y("2^3^2", 0.0), 512.0);
        assert_eq!(y("x - floor(x)", -1.25), 0.75);
        assert_eq!(y("-7 % 3", 0.0), 2.0);
        assert_eq!(y("7 // 2", 0.0), 3.0);
        assert_eq!(y("max(x, 1) + min(x, 1)", 4.0), 5.0);
        assert!((y("sin(pi/2) * e", 0.0) - std::f64::consts::E).abs() < 1e-12);
        assert_eq!(y("1e2 + .5", 0.0), 100.5);
        assert!(y("log(x)", -1.0).is_nan());
        assert!(Formula::parse("sin(x - t)").unwrap().uses_t);
    }

    /// Typos, unknown names and functions, wrong argument counts and stray characters are refused.
    #[test]
    fn refuses_mistakes() {
        for bad in ["", "sin(x", "foo(x)", "y + 1", "sin(x, x)", "max(x)", "2x", "x $ 2", "import os"] {
            assert!(Formula::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Formula::parse("max(x)").unwrap_err().0, "max() takes 2 numbers, like max(x, 1).");
    }
}
