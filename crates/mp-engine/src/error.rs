use std::fmt;

#[derive(Debug)]
pub enum Error {
    MuPdf(mupdf::Error),
    UnknownDocument,
    /// The engine or render pool thread is gone (it panicked or was shut down).
    Stopped,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MuPdf(e) => write!(f, "{e}"),
            Error::UnknownDocument => f.write_str("document is not open"),
            Error::Stopped => f.write_str("PDF engine has stopped"),
        }
    }
}

impl std::error::Error for Error {}

impl From<mupdf::Error> for Error {
    fn from(e: mupdf::Error) -> Self {
        Error::MuPdf(e)
    }
}
