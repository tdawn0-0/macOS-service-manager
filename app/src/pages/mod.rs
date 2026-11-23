//! One module per sidebar section. Each page composes the app's components;
//! `store` holds what it reads and changes.

mod databases;
mod development;
mod diagnostics;
mod https_proxy;
mod ports;
mod services;
mod settings;
mod software;

pub(crate) use databases::databases;
pub(crate) use development::development;
pub(crate) use diagnostics::diagnostics;
pub(crate) use https_proxy::https_proxy;
pub(crate) use ports::ports;
pub(crate) use services::services;
pub(crate) use settings::settings;
