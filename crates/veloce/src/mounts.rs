//! The FUSE mounts of saved connections: which are up, which are being set up, and
//! the folder each one lives in.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use veloce_mount::Mounted;
use veloce_store::SavedConnection;

use crate::connections;

/// Where mounted connections appear.
pub fn mount_root(home: &Path) -> PathBuf {
    home.join("Veloce")
}

struct Active {
    mounted: Mounted,
    folder: String,
}

#[derive(Default)]
pub struct Mounts {
    active: HashMap<i64, Active>,
    /// Being mounted right now, so a second sync does not start them again.
    pending: HashMap<i64, String>,
}

/// What a sync has to do to make the mounts match the saved connections.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub unmount: Vec<i64>,
    pub mount: Vec<i64>,
}

/// Compare the connections that want a mount with the ones that have one (or are
/// getting one).
pub fn plan(wanted: &HashSet<i64>, have: &HashSet<i64>) -> Plan {
    let mut unmount: Vec<i64> = have.difference(wanted).copied().collect();
    let mut mount: Vec<i64> = wanted.difference(have).copied().collect();
    unmount.sort_unstable();
    mount.sort_unstable();
    Plan { unmount, mount }
}

impl Mounts {
    pub fn wanted(connections: &[SavedConnection]) -> HashSet<i64> {
        connections
            .iter()
            .filter(|connection| connections::wants_mount(&connection.config))
            .map(|connection| connection.id)
            .collect()
    }

    fn have(&self) -> HashSet<i64> {
        self.active
            .keys()
            .chain(self.pending.keys())
            .copied()
            .collect()
    }

    pub fn plan(&self, connections: &[SavedConnection]) -> Plan {
        plan(&Self::wanted(connections), &self.have())
    }

    /// Reserve a folder name for `id` and mark it as being mounted.
    pub fn begin(&mut self, id: i64, name: &str) -> String {
        let taken: Vec<String> = self
            .active
            .values()
            .map(|active| active.folder.clone())
            .chain(self.pending.values().cloned())
            .collect();
        let folder = connections::mount_folder_name(name, &taken);
        self.pending.insert(id, folder.clone());
        folder
    }

    pub fn finish(&mut self, id: i64, mounted: Option<Mounted>) {
        let folder = self.pending.remove(&id);
        if let (Some(mounted), Some(folder)) = (mounted, folder) {
            self.active.insert(id, Active { mounted, folder });
        }
    }

    /// Take a mount out so the caller can drop (unmount) it off the UI thread.
    pub fn take(&mut self, id: i64) -> Option<Mounted> {
        self.pending.remove(&id);
        self.active.remove(&id).map(|active| active.mounted)
    }

    pub fn take_all(&mut self) -> Vec<Mounted> {
        self.pending.clear();
        self.active.drain().map(|(_, active)| active.mounted).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[i64]) -> HashSet<i64> {
        ids.iter().copied().collect()
    }

    #[test]
    fn a_new_wish_is_mounted_and_a_dropped_one_unmounted() {
        let result = plan(&set(&[1, 2]), &set(&[2, 3]));
        assert_eq!(result.mount, [1]);
        assert_eq!(result.unmount, [3]);
    }

    #[test]
    fn matching_sets_need_no_work() {
        let result = plan(&set(&[1, 2]), &set(&[1, 2]));
        assert!(result.mount.is_empty() && result.unmount.is_empty());
        let none = plan(&set(&[]), &set(&[]));
        assert!(none.mount.is_empty() && none.unmount.is_empty());
    }

    #[test]
    fn mounts_live_in_a_veloce_folder_under_home() {
        assert_eq!(
            mount_root(Path::new("/Users/me")),
            PathBuf::from("/Users/me/Veloce")
        );
    }

    #[test]
    fn a_connection_being_mounted_is_not_started_twice_and_names_do_not_clash() {
        let mut mounts = Mounts::default();
        let connection = |id: i64, name: &str| SavedConnection {
            id,
            kind: "webdav".into(),
            name: name.into(),
            config: r#"{"url":"https://d.example.com","mount":true}"#.into(),
            created_at: 0,
        };
        let list = [connection(1, "docs"), connection(2, "docs")];
        assert_eq!(mounts.plan(&list).mount, [1, 2]);
        assert_eq!(mounts.begin(1, "docs"), "docs");
        assert_eq!(mounts.plan(&list).mount, [2], "1 is already in progress");
        assert_eq!(mounts.begin(2, "docs"), "docs 2", "same name, another folder");
        mounts.finish(1, None);
        assert_eq!(mounts.plan(&list).mount, [1], "a failed mount can be retried");
    }

    #[test]
    fn connections_without_the_flag_are_never_wanted() {
        let plain = SavedConnection {
            id: 1,
            kind: "ssh".into(),
            name: "a".into(),
            config: r#"{"host":"h","user":"u"}"#.into(),
            created_at: 0,
        };
        assert!(Mounts::wanted(&[plain]).is_empty());
    }
}
