// placeholder
use zeroize::Zeroizing;
use crate::state::PinentryState;
use crate::error::{GPG_ERR_CANCELED};

pub fn show_getpin(_state: &PinentryState) -> Result<Zeroizing<String>, u32> {
    Err(GPG_ERR_CANCELED)
}

pub fn show_confirm(_state: &PinentryState) -> Result<bool, u32> {
    Err(GPG_ERR_CANCELED)
}
