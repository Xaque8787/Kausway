//! Optional discovery of encrypted invitations. Normal invitation URLs bypass this crate.
pub mod backend;
mod code;
mod record;

pub use code::{Code, Secrets};
pub use iroh_dns::pkarr::SignedPacket;
pub use record::{
    CLOCK_SKEW_SECS, MAX_RECORD_AGE_SECS, RECORD_TTL_SECS, SealedInvite, build_packet,
    decode_packet, fit_packet,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid or unsupported short code")]
    InvalidCode,
    #[error("invalid rendezvous record")]
    InvalidRecord,
    #[error("rendezvous record is expired or stale")]
    Expired,
    #[error("invitation cannot fit a rendezvous record with a usable address")]
    TooLarge,
    #[error("rendezvous backend configuration is invalid")]
    Configuration,
    #[error("rendezvous network operation failed")]
    Network,
    #[error("rendezvous record was not found")]
    NotFound,
    #[error("rendezvous operation timed out")]
    Timeout,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidCode => "invalid_code",
            Self::InvalidRecord => "invalid_record",
            Self::Expired => "expired",
            Self::TooLarge => "too_large",
            Self::Configuration => "configuration",
            Self::Network => "network",
            Self::NotFound => "not_found",
            Self::Timeout => "timeout",
        }
    }
}
