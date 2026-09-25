use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("microphone: {0}")]
    Microphone(String),
    #[error("meeting audio: {0}")]
    OtherSide(String),
    #[error("encoder: {0}")]
    Encoder(String),
    #[error("{0}")]
    Unsupported(&'static str),
}
