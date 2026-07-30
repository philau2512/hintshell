pub mod dynamic;
pub mod generator;
pub mod path;
pub mod process;
pub mod workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub input: String,
    pub tokens: Vec<String>,
    pub command: Option<String>,
    pub arguments: Vec<String>,
    pub current_token: String,
    pub has_trailing_space: bool,
}

impl CommandContext {
    pub fn parse(input: impl Into<String>) -> Self {
        let input = input.into();
        let tokens = tokenize_command_line(&input);
        let has_trailing_space = ends_with_unquoted_whitespace(&input);
        let command = tokens.first().cloned();
        let arguments = tokens.iter().skip(1).cloned().collect();
        let current_token = if has_trailing_space {
            String::new()
        } else {
            tokens.last().cloned().unwrap_or_default()
        };

        Self {
            input,
            tokens,
            command,
            arguments,
            current_token,
            has_trailing_space,
        }
    }
}

pub fn tokenize_command_line(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut token_started = false;

    for character in input.chars() {
        if escaped {
            token.push(character);
            token_started = true;
            escaped = false;
            continue;
        }

        match (quote, character) {
            (_, '\\') => escaped = true,
            (Some(active), character) if character == active => quote = None,
            (None, '\'' | '"') => {
                quote = Some(character);
                token_started = true;
            }
            (None, character) if character.is_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            _ => {
                token.push(character);
                token_started = true;
            }
        }
    }

    if escaped {
        token.push('\\');
    }
    if token_started {
        tokens.push(token);
    }

    tokens
}

fn ends_with_unquoted_whitespace(input: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut last = None;

    for character in input.chars() {
        if escaped {
            escaped = false;
            last = Some(character);
            continue;
        }
        match (quote, character) {
            (_, '\\') => escaped = true,
            (Some(active), character) if character == active => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            _ => {}
        }
        last = Some(character);
    }

    quote.is_none() && !escaped && last.is_some_and(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quotes_escapes_and_trailing_space() {
        let context = CommandContext::parse("git commit -m \"hello world\" file\\ name ");

        assert_eq!(
            context.tokens,
            ["git", "commit", "-m", "hello world", "file name"]
        );
        assert!(context.has_trailing_space);
        assert_eq!(context.current_token, "");
    }

    #[test]
    fn preserves_unfinished_quote_as_current_token() {
        let context = CommandContext::parse("cat \"src/main");

        assert_eq!(context.command.as_deref(), Some("cat"));
        assert_eq!(context.current_token, "src/main");
        assert!(!context.has_trailing_space);
    }
}
