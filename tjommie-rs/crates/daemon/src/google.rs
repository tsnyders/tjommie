//! `google.*`. The separate Google API feature is on hold: in production every method answers that it isn't enabled
//! (`daemon.ts:171-172`), and bots use the native browser instead.

use protocol::ErrorCode;
use protocol::wire::method;

use crate::server::{Handlers, RpcError};

const NOT_ENABLED: &str =
    "The separate Google API connection is not enabled in this version. Use the native browser.";

fn not_enabled<T>() -> Result<T, RpcError> {
    Err(RpcError::new(ErrorCode::BadState, NOT_ENABLED))
}

pub fn register(handlers: Handlers) -> Handlers {
    handlers
        .on::<method::GoogleStatus, _>(|_, _| async { not_enabled() })
        .on::<method::GoogleConfigure, _>(|_, _| async { not_enabled() })
        .on::<method::GoogleBegin, _>(|_, _| async { not_enabled() })
        .on::<method::GoogleCancel, _>(|_, _| async { not_enabled() })
        .on::<method::GoogleDisconnect, _>(|_, _| async { not_enabled() })
        .on::<method::GoogleGrant, _>(|_, _| async { not_enabled() })
}
