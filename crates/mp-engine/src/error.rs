use std::fmt;

#[derive(Debug)]
pub enum Error {
    MuPdf(mupdf::Error),
    UnknownDocument,
    /// The requested item (an attachment, say) does not exist in the document.
    NotFound,
    /// The action needs a PDF, and the document is another format.
    NotPdf,
    /// The request itself is malformed.
    Invalid(&'static str),
    Io(std::io::Error),
    /// The engine or render pool thread is gone (it panicked or was shut down).
    Stopped,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MuPdf(e) => write!(f, "{e}"),
            Error::UnknownDocument => f.write_str("document is not open"),
            Error::NotFound => f.write_str("not found in the document"),
            Error::NotPdf => f.write_str("this needs a PDF document"),
            Error::Invalid(why) => f.write_str(why),
            Error::Io(e) => write!(f, "{e}"),
            Error::Stopped => f.write_str("PDF engine has stopped"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<mupdf::Error> for Error {
    fn from(e: mupdf::Error) -> Self {
        Error::MuPdf(e)
    }
}
