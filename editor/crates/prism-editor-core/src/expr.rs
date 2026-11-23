//! Expression evaluation in numeric input boxes (equivalent to Unity's `EditorGUI.NumberFieldValue`).
//!
//! Numbers in the inspector could only be typed as literals: to fill half of 90° you had to compute 45 yourself. Unity's
//! number box accepts expressions (`90/2`, `2*3`, `sin(30)`), this implements the same thing.
//!
//! Pure arithmetic only, no script engine: numbers, `+ - * / %`, `^` (power, right-associative), parentheses,
//! unary plus/minus, plus some constants and functions. **Trigonometric functions use degrees**, consistent with `°` shown in inspector.
//!
//! ```text
//! evaluate("90/2")            => Some(45.0)
//! evaluate("2*(3+4)")         => Some(14.0)
//! evaluate("sin(30)")         => Some(0.5)
//! evaluate("min(1,2)+1")      => Some(2.0)
//! evaluate("90/")             => None
//! ```
//!
//! Return value must be finite: `1/0`, `sqrt(-1)` all `None`, letting caller retain original value.

/// Unified return type when evaluation fails.
pub type EvalResult = Option<f64>;

/// Parse and evaluate an expression; syntax errors, unknown functions/constants, and non-finite results all return `None`.
pub fn evaluate(input: &str) -> EvalResult {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expression()?;
    if parser.pos != parser.tokens.len() {
        return None;
    }
    value.is_finite().then_some(value)
}

/// Fast path when the input box contains only a number (the most common case), skipping tokenization allocation.
pub fn parse_number_or_expression(input: &str) -> EvalResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        return value.is_finite().then_some(value);
    }
    evaluate(trimmed)
}


#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Open,
    Close,
    Comma,
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        match ch {
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => tokens.push(Token::Star),
            '/' => tokens.push(Token::Slash),
            '%' => tokens.push(Token::Percent),
            '^' => tokens.push(Token::Caret),
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            ',' => tokens.push(Token::Comma),
            _ if ch.is_ascii_digit() || ch == '.' => {
                let start = index;
                let mut seen_dot = false;
                let mut seen_exp = false;
                while index < chars.len() {
                    let current = chars[index];
                    if current.is_ascii_digit() {
                        index += 1;
                    } else if current == '.' && !seen_dot && !seen_exp {
                        seen_dot = true;
                        index += 1;
                    } else if (current == 'e' || current == 'E') && !seen_exp && index > start {
                        seen_exp = true;
                        index += 1;
                        if index < chars.len() && (chars[index] == '+' || chars[index] == '-') {
                            index += 1;
                        }
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..index].iter().collect();
                tokens.push(Token::Number(text.parse().ok()?));
                continue;
            }
            _ if ch.is_alphabetic() || ch == '_' => {
                let start = index;
                while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_')
                {
                    index += 1;
                }
                let text: String = chars[start..index].iter().collect();
                tokens.push(Token::Ident(text.to_lowercase()));
                continue;
            }
            _ => return None,
        }
        index += 1;
    }
    (!tokens.is_empty()).then_some(tokens)
}


struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn expression(&mut self) -> EvalResult {
        let mut value = self.term()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.pos += 1;
                    value += self.term()?;
                }
                Some(Token::Minus) => {
                    self.pos += 1;
                    value -= self.term()?;
                }
                _ => return Some(value),
            }
        }
    }

    fn term(&mut self) -> EvalResult {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.pos += 1;
                    value *= self.unary()?;
                }
                Some(Token::Slash) => {
                    self.pos += 1;
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value /= divisor;
                }
                Some(Token::Percent) => {
                    self.pos += 1;
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value %= divisor;
                }
                _ => return Some(value),
            }
        }
    }

    fn unary(&mut self) -> EvalResult {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(Token::Plus) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> EvalResult {
        let base = self.primary()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.pos += 1;
            let exponent = self.unary()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn primary(&mut self) -> EvalResult {
        match self.next()? {
            Token::Number(value) => Some(value),
            Token::Open => {
                let value = self.expression()?;
                if !matches!(self.next(), Some(Token::Close)) {
                    return None;
                }
                Some(value)
            }
            Token::Ident(name) => self.ident(&name),
            _ => None,
        }
    }

    fn ident(&mut self, name: &str) -> EvalResult {
        if !matches!(self.peek(), Some(Token::Open)) {
            return constant(name);
        }
        self.pos += 1; 
        let mut args = Vec::new();
        if matches!(self.peek(), Some(Token::Close)) {
            self.pos += 1;
        } else {
            loop {
                args.push(self.expression()?);
                match self.next()? {
                    Token::Comma => continue,
                    Token::Close => break,
                    _ => return None,
                }
            }
        }
        function(name, &args)
    }
}

fn constant(name: &str) -> EvalResult {
    match name {
        "pi" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        "tau" => Some(std::f64::consts::TAU),
        _ => None,
    }
}

/// Function table. Trigonometric functions take/return **degrees**, consistent with `°` shown in the inspector.
fn function(name: &str, args: &[f64]) -> EvalResult {
    let unary = |expected: usize| (args.len() == expected).then_some(());
    match name {
        "sin" => unary(1).and_then(|_| Some(args[0].to_radians().sin())),
        "cos" => unary(1).and_then(|_| Some(args[0].to_radians().cos())),
        "tan" => unary(1).and_then(|_| Some(args[0].to_radians().tan())),
        "asin" => unary(1).map(|_| args[0].asin().to_degrees()),
        "acos" => unary(1).map(|_| args[0].acos().to_degrees()),
        "atan" => unary(1).map(|_| args[0].atan().to_degrees()),
        "sqrt" => unary(1).and_then(|_| (args[0] >= 0.0).then(|| args[0].sqrt())),
        "abs" => unary(1).map(|_| args[0].abs()),
        "floor" => unary(1).map(|_| args[0].floor()),
        "ceil" => unary(1).map(|_| args[0].ceil()),
        "round" => unary(1).map(|_| args[0].round()),
        "min" => (!args.is_empty()).then(|| args.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => (!args.is_empty()).then(|| args.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "pow" => unary(2).map(|_| args[0].powf(args[1])),
        "clamp" => unary(3).map(|_| args[0].clamp(args[1].min(args[2]), args[1].max(args[2]))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(input: &str) -> f64 {
        evaluate(input).unwrap_or_else(|| panic!("{input:?} should parse"))
    }

    #[test]
    fn plain_numbers_and_arithmetic() {
        assert_eq!(value("42"), 42.0);
        assert_eq!(value(" -3.5 "), -3.5);
        assert_eq!(value("1e3"), 1000.0);
        assert_eq!(value("2+3*4"), 14.0);
        assert_eq!(value("(2+3)*4"), 20.0);
        assert_eq!(value("90/2"), 45.0);
        assert_eq!(value("7%4"), 3.0);
        assert_eq!(value("2^3^2"), 512.0, "powers are right-associative");
        assert_eq!(value("-(2+3)"), -5.0);
    }

    #[test]
    fn constants_and_functions_use_degrees() {
        assert!((value("sin(30)") - 0.5).abs() < 1e-12);
        assert!((value("cos(60)") - 0.5).abs() < 1e-12);
        assert!((value("pi") - std::f64::consts::PI).abs() < 1e-12);
        assert_eq!(value("min(1,2)+1"), 2.0);
        assert_eq!(value("max(1,2)"), 2.0);
        assert_eq!(value("clamp(5,0,3)"), 3.0);
        assert_eq!(value("sqrt(16)"), 4.0);
        assert_eq!(value("pow(2,10)"), 1024.0);
    }

    #[test]
    fn broken_input_is_rejected_instead_of_guessed() {
        for input in [
            "", "   ", "90/", "1 2", "(1+2", "1+", "nope(1)", "sin()", "sin(1,2)", "1/0", "10%0",
            "sqrt(-1)", "@", "2^^3",
        ] {
            assert!(evaluate(input).is_none(), "{input:?} must not evaluate");
        }
    }

    #[test]
    fn parse_number_or_expression_fast_paths_plain_values() {
        assert_eq!(parse_number_or_expression("12.5"), Some(12.5));
        assert_eq!(parse_number_or_expression(" 90/2 "), Some(45.0));
        assert_eq!(parse_number_or_expression(""), None);
        assert_eq!(parse_number_or_expression("abc"), None);
        assert_eq!(evaluate("1e308*10"), None);
    }
}
