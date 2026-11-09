//! `calc()` for length and number declarations. Supports `+ - * /`, parentheses and
//! unitless numbers, `px` and `%`. Mixed units in `+`/`-`, length*length and division by a
//! length or zero are rejected at compile time.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CalcUnit {
    Number,
    Px,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalcValue {
    pub value: f32,
    pub unit: CalcUnit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Value(CalcValue),
    Op(char),
    Open,
    Close,
}

/// Evaluates a whole `calc(...)` value.
pub fn eval_calc(input: &str) -> Result<CalcValue, String> {
    let inner = input
        .trim()
        .strip_prefix("calc(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| format!("'{}' is not a single calc() value", input.trim()))?;
    let tokens = tokenize(inner)?;
    let mut parser = Parser {
        tokens: &tokens,
        cursor: 0,
        depth: 0,
    };
    let value = parser.expr()?;
    if parser.cursor != tokens.len() {
        return Err(format!("unexpected token in calc({inner})"));
    }
    if !value.value.is_finite() {
        return Err(format!("calc({inner}) is not finite"));
    }
    Ok(value)
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => cursor += 1,
            b'(' => {
                tokens.push(Token::Open);
                cursor += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                cursor += 1;
            }
            b'*' | b'/' => {
                tokens.push(Token::Op(byte as char));
                cursor += 1;
            }
            b'+' | b'-' if !expects_operand(tokens.last()) => {
                tokens.push(Token::Op(byte as char));
                cursor += 1;
            }
            b'+' | b'-' | b'0'..=b'9' | b'.' => {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len() && matches!(bytes[cursor], b'0'..=b'9' | b'.') {
                    cursor += 1;
                }
                let number: f32 = input[start..cursor]
                    .parse()
                    .map_err(|_| format!("invalid number '{}' in calc()", &input[start..cursor]))?;
                let unit = if input[cursor..].starts_with("px") {
                    cursor += 2;
                    CalcUnit::Px
                } else if input[cursor..].starts_with('%') {
                    cursor += 1;
                    CalcUnit::Percent
                } else {
                    CalcUnit::Number
                };
                tokens.push(Token::Value(CalcValue {
                    value: number,
                    unit,
                }));
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                // Only a nested `calc(` is accepted; any other function or keyword is unsupported.
                let start = cursor;
                while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
                    cursor += 1;
                }
                if &input[start..cursor] != "calc" || bytes.get(cursor) != Some(&b'(') {
                    return Err(format!(
                        "unsupported term '{}' in calc()",
                        &input[start..cursor]
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "unsupported character '{}' in calc()",
                    byte as char
                ))
            }
        }
    }
    Ok(tokens)
}

/// True when the next token must be an operand, so a `+`/`-` there is a sign.
fn expects_operand(previous: Option<&Token>) -> bool {
    matches!(previous, None | Some(Token::Op(_)) | Some(Token::Open))
}

/// Deepest `(` nesting accepted in one calc(); deeper input is rejected, not recursed into.
const MAX_CALC_DEPTH: usize = 64;

struct Parser<'a> {
    tokens: &'a [Token],
    cursor: usize,
    depth: usize,
}

impl Parser<'_> {
    fn expr(&mut self) -> Result<CalcValue, String> {
        let mut left = self.term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek() {
            self.cursor += 1;
            let right = self.term()?;
            left = add_or_sub(left, right, op)?;
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<CalcValue, String> {
        let mut left = self.factor()?;
        while let Some(Token::Op(op @ ('*' | '/'))) = self.peek() {
            self.cursor += 1;
            let right = self.factor()?;
            left = if op == '*' {
                multiply(left, right)?
            } else {
                divide(left, right)?
            };
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<CalcValue, String> {
        match self.peek() {
            Some(Token::Value(value)) => {
                self.cursor += 1;
                Ok(value)
            }
            Some(Token::Open) => {
                if self.depth == MAX_CALC_DEPTH {
                    return Err(format!("calc() nests deeper than {MAX_CALC_DEPTH} levels"));
                }
                self.cursor += 1;
                self.depth += 1;
                let value = self.expr()?;
                self.depth -= 1;
                if self.peek() != Some(Token::Close) {
                    return Err("calc() has an unclosed '('".to_string());
                }
                self.cursor += 1;
                Ok(value)
            }
            _ => Err("calc() expects a number or '('".to_string()),
        }
    }

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.cursor).copied()
    }
}

fn add_or_sub(left: CalcValue, right: CalcValue, op: char) -> Result<CalcValue, String> {
    if left.unit != right.unit {
        return Err(format!(
            "calc() cannot combine {:?} and {:?} with '{op}'",
            left.unit, right.unit
        ));
    }
    let value = if op == '+' {
        left.value + right.value
    } else {
        left.value - right.value
    };
    Ok(CalcValue {
        value,
        unit: left.unit,
    })
}

fn multiply(left: CalcValue, right: CalcValue) -> Result<CalcValue, String> {
    let unit = match (left.unit, right.unit) {
        (CalcUnit::Number, unit) | (unit, CalcUnit::Number) => unit,
        _ => return Err("calc() cannot multiply two lengths".to_string()),
    };
    Ok(CalcValue {
        value: left.value * right.value,
        unit,
    })
}

fn divide(left: CalcValue, right: CalcValue) -> Result<CalcValue, String> {
    if right.unit != CalcUnit::Number || right.value == 0.0 {
        return Err("calc() divides only by a non-zero number".to_string());
    }
    Ok(CalcValue {
        value: left.value / right.value,
        unit: left.unit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(input: &str) -> CalcValue {
        eval_calc(input).unwrap_or_else(|error| panic!("{input}: {error}"))
    }

    #[test]
    fn evaluates_px_percent_and_number_arithmetic_with_precedence() {
        assert_eq!(
            value("calc(10px + 2px * 3)"),
            CalcValue {
                value: 16.0,
                unit: CalcUnit::Px
            }
        );
        assert_eq!(
            value("calc((10px + 2px) / 4)").value,
            3.0,
            "parentheses group before division"
        );
        assert_eq!(value("calc(50% - 10%)").unit, CalcUnit::Percent);
        assert_eq!(value("calc(-4px + 10px)").value, 6.0, "leading sign");
        assert_eq!(value("calc(2 * 3)").unit, CalcUnit::Number);
        assert_eq!(value("calc(100% / 4)").value, 25.0);
    }

    #[test]
    fn nested_calc_is_accepted() {
        assert_eq!(value("calc(calc(4px + 2px) * 2)").value, 12.0);
    }

    #[test]
    fn rejects_invalid_calc_expressions() {
        for bad in [
            "10px",
            "calc(10px + 5%)",
            "calc(10px * 2px)",
            "calc(10px / 0)",
            "calc(10px / 2px)",
            "calc(10px +)",
            "calc((10px)",
            "calc(10px))",
            "calc(10em)",
            "calc(min(1px, 2px))",
            "calc(1px) + 2px",
            "calc()",
            "calc(1.2.3px)",
        ] {
            assert!(eval_calc(bad).is_err(), "{bad} must be rejected");
        }
    }
}
