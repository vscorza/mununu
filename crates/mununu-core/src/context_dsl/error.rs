use crate::context_dsl::token::Span;

#[derive(Debug, thiserror::Error)]
pub enum LexError {
    #[error("unexpected character `{ch}` at {span:?}")]
    UnexpectedChar { ch: char, span: Span },
    #[error("unterminated string literal at {span:?}")]
    UnterminatedString { span: Span },
    #[error("unterminated block comment at {span:?}")]
    UnterminatedComment { span: Span },
    #[error("integer literal out of range at {span:?}")]
    IntegerOverflow { span: Span },
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Lex(#[from] LexError),
    #[error("unexpected end of input")]
    Eof,
    #[error("unexpected token {found:?}, expected {expected}")]
    UnexpectedToken {
        found: crate::context_dsl::token::TokenKind,
        expected: &'static str,
        span: Span,
    },
    #[error("duplicate declaration `{name}` at {span:?}")]
    DuplicateItem { name: String, span: Span },
    #[error("invalid expression at {span:?}: {message}")]
    InvalidExpr { span: Span, message: String },
}
