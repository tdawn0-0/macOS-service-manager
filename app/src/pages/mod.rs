//! One module per sidebar section. Each page composes the app's components;
//! `store` holds what it reads and changes.

mod databases;
mod diagnostics;
mod https_proxy;
mod services;
mod settings;

pub(crate) use databases::databases;
pub(crate) use diagnostics::diagnostics;
pub(crate) use https_proxy::https_proxy;
pub(crate) use services::services;
pub(crate) use settings::settings;
