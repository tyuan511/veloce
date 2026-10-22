//! Glue between saved connections and the running app: opening one into a provider,
//! and telling every window when the saved list changed.

use std::sync::Arc;

use gpui::{App, Global};
use veloce_store::SavedConnection;
use vfs_core::{RemoteProvider, VfsUri};

use crate::{
    connections::{self, Kind},
    i18n::trf,
    secrets,
};

/// Bumped whenever a connection is added or removed, so windows that show the list
/// (the sidebar) reload without the Settings window knowing about them.
#[derive(Default)]
pub struct ConnectionsRevision {
    counter: u64,
    /// A connection whose settings or secret just changed: any provider already
    /// connected with the old ones has to be dropped.
    pub edited: Option<i64>,
}

impl Global for ConnectionsRevision {}

pub fn notify_changed(cx: &mut App, edited: Option<i64>) {
    let counter = cx
        .try_global::<ConnectionsRevision>()
        .map_or(0, |r| r.counter)
        + 1;
    cx.set_global(ConnectionsRevision { counter, edited });
}

/// What the registry needs to mount a connection.
pub struct Mounted {
    pub id: String,
    pub provider: Arc<RemoteProvider>,
}

/// The id a connection carries inside its uris. The database id, not the name, so
/// renaming never invalidates open tabs.
pub fn uri_id(connection: &SavedConnection) -> String {
    connection.id.to_string()
}

pub fn root_uri(connection: &SavedConnection) -> Option<VfsUri> {
    let kind = Kind::from_key(&connection.kind)?;
    Some(VfsUri::remote(kind.scheme(), uri_id(connection), "/"))
}

/// Read the secret from the keychain and build the provider. Blocks on the keychain,
/// so call it off the UI thread.
pub fn mount(connection: &SavedConnection) -> Result<Mounted, String> {
    let kind = Kind::from_key(&connection.kind)
        .ok_or_else(|| trf("Unknown connection type {}", &[&connection.kind]))?;
    let secret = secrets::load(connection.id)
        .map_err(|error| trf("Cannot read the keychain: {}", &[&error]))?;
    let spec = connections::to_spec(kind, &connection.config, secret)?;
    let provider = RemoteProvider::connect(&spec).map_err(|error| error.to_string())?;
    Ok(Mounted {
        id: uri_id(connection),
        provider: Arc::new(provider),
    })
}
