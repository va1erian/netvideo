//! Authentication: server keys, PASETO tokens and device pairing.

pub mod keys;
pub mod link;
pub mod middleware;
pub mod pairing;
pub mod paseto;

pub use keys::ServerKey;
pub use middleware::{AdminDevice, AuthDevice, ClientIp};
pub use pairing::{PairOutcome, pair};
pub use paseto::{IssuedToken, VerifiedAccess, issue_access_token, verify_access_token};
