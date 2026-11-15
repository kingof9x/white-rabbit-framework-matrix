//! Formulas typed on the Math page, like `sin(x) * x^2` (`compile_formula` in `math_sims.py`).
//!
//! Only numbers, x, t, pi, e, + - * / % // ^ ** and a list of maths functions are allowed. Anything
//! that can't be worked out (log of a negative number, 1/0...) gives NaN, which the graph leaves out.

#[derive(Debug, Clone, PartialEq)]
pub struct FormulaError(pub String);

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    FloorDiv,
    Pow,
}

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    X,
    T,
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(&'static str, Vec<Node>),
}

const ONE_ARGUMENT: [&str; 21] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "exp", "log", "ln", "log10", "sqrt", "abs",
    "floor", "ceil", "round", "sign", "sinc", "trunc",
];
const TWO_ARGUMENTS: [&str; 2] = ["min", "max"];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
}

fn typo() -> FormulaError {
    FormulaError("That formula has a typo in it.".into())
}

fn tokens(text: &str) -> Result<Vec<Tok>, FormulaError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // An exponent like 1e3 or 2.5E-2
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
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
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
        } else {
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

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

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

    fn expr(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.term()?;
        while let Some(o) = self.eat_op(&["+", "-"]) {
            let right = self.term()?;
            left = Node::Bin(if o == "+" { Op::Add } else { Op::Sub }, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

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

    fn unary(&mut self) -> Result<Node, FormulaError> {
        if let Some(o) = self.eat_op(&["-", "+"]) {
            let inner = self.unary()?;
            return Ok(if o == "-" { Node::Neg(Box::new(inner)) } else { inner });
        }
        self.power()
    }

    fn power(&mut self) -> Result<Node, FormulaError> {
        let base = self.atom()?;
        if self.eat_op(&["**"]).is_some() {
            // Right to left, and -x^2 is -(x^2), as in Python
            let exponent = self.unary()?;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, FormulaError> {
        let tok = self.peek().cloned().ok_or_else(typo)?;
        self.at += 1;
        match tok {
            Tok::Num(n) => Ok(Node::Num(n)),
            Tok::Open => {
                let inner = self.expr()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err(typo());
                }
                self.at += 1;
                Ok(inner)
            }
            Tok::Name(name) => {
                if self.peek() == Some(&Tok::Open) {
                    self.at += 1;
                    let func =
                        ONE_ARGUMENT.iter().chain(TWO_ARGUMENTS.iter()).find(|f| **f == name).copied().ok_or_else(
                            || FormulaError("Only functions like sin(x) or sqrt(x) can be called.".into()),
                        )?;
                    let mut args = Vec::new();
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
                match name.as_str() {
                    "x" => Ok(Node::X),
                    "t" => Ok(Node::T),
                    "pi" => Ok(Node::Num(std::f64::consts::PI)),
                    "e" => Ok(Node::Num(std::f64::consts::E)),
                    _ => Err(FormulaError(format!("I don't know '{name}'. Use x, t, pi, e and functions like sin."))),
                }
            }
            _ => Err(typo()),
        }
    }
}

/// A formula ready to work out.
#[derive(Debug, Clone)]
pub struct Formula {
    root: Node,
    pub uses_t: bool,
}

fn uses_t(n: &Node) -> bool {
    match n {
        Node::T => true,
        Node::Neg(a) => uses_t(a),
        Node::Bin(_, a, b) => uses_t(a) || uses_t(b),
        Node::Call(_, args) => args.iter().any(uses_t),
        _ => false,
    }
}

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

fn eval(n: &Node, x: f64, t: f64) -> f64 {
    match n {
        Node::Num(v) => *v,
        Node::X => x,
        Node::T => t,
        Node::Neg(a) => -eval(a, x, t),
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
                "round" => a.round_ties_even(),
                "trunc" => a.trunc(),
                "sign" => {
                    if a > 0.0 {
                        1.0
                    } else if a < 0.0 {
                        -1.0
                    } else {
                        a
                    }
                }
                "sinc" => {
                    if a == 0.0 {
                        1.0
                    } else {
                        a.sin() / a
                    }
                }
                "min" => a.min(eval(&args[1], x, t)),
                "max" => a.max(eval(&args[1], x, t)),
                _ => f64::NAN,
            }
        }
    }
}

impl Formula {
    /// Reads text like 'sin(x) * x^2'.
    pub fn parse(text: &str) -> Result<Formula, FormulaError> {
        let source = text.trim();
        if source.is_empty() {
            return Err(FormulaError("Type a formula, for example sin(x).".into()));
        }
        if source.chars().count() > 200 {
            return Err(FormulaError("That formula is too long.".into()));
        }
        let mut p = Parser { toks: tokens(source)?, at: 0 };
        let root = p.expr()?;
        if p.at != p.toks.len() {
            return Err(typo());
        }
        let uses_t = uses_t(&root);
        Ok(Formula { root, uses_t })
    }

    pub fn at(&self, x: f64, t: f64) -> f64 {
        eval(&self.root, x, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(text: &str, x: f64) -> f64 {
        Formula::parse(text).unwrap().at(x, 0.0)
    }

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

    #[test]
    fn refuses_mistakes() {
        for bad in ["", "sin(x", "foo(x)", "y + 1", "sin(x, x)", "max(x)", "2x", "x $ 2", "import os"] {
            assert!(Formula::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Formula::parse("max(x)").unwrap_err().0, "max() takes 2 numbers, like max(x, 1).");
    }
}
