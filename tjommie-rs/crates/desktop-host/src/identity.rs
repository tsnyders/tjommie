//! Which app this binary is (spec §4, "Coexistence identity"). `TJOMMIE_IDENTITY` picks it at build time through
//! build.rs: `dev` when unset, `production`, `updatetest` or `e2e`. A dev or e2e binary can't become production at
//! run time, and only the production identity carries the update feed (spec §3.10).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// The TJOMMIE_IDENTITY value.
    pub name: &'static str,
    pub app_id: &'static str,
    /// The product name: window title, shortcuts, the installer.
    pub product: &'static str,
    /// The installed exe's name.
    pub exe: &'static str,
    pub data_root: DataRoot,
    pub ports: Ports,
    /// GitHub Releases `latest.yml` for the updater (T6.4); only production has one.
    pub update_feed: Option<&'static str>,
    /// Start at login (T6.3).
    pub login_item: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRoot {
    /// This folder under %LOCALAPPDATA%.
    LocalAppData(&'static str),
    /// A disposable folder under %TEMP%, given as TJOMMIE_HOME (tests, the update test).
    Disposable,
}

/// The daemon's ports: the app's and the phone's (LAN). 0 asks for any free port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub app: u16,
    pub lan: u16,
}

pub const PRODUCTION: Identity = Identity {
    name: "production",
    app_id: "com.tsnyders.tjommie",
    product: "Tjommie",
    exe: "Tjommie.exe",
    data_root: DataRoot::LocalAppData("Tjommie"),
    ports: Ports {
        app: 47100,
        lan: 47101,
    },
    update_feed: Some("https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml"),
    login_item: true,
};

pub const DEV: Identity = Identity {
    name: "dev",
    app_id: "com.tsnyders.tjommie-rs",
    product: "Tjommie RS",
    exe: "TjommieRS.exe",
    data_root: DataRoot::LocalAppData("Tjommie-rs"),
    ports: Ports {
        app: 47200,
        lan: 47201,
    },
    update_feed: None,
    login_item: false,
};

/// The isolated update-test app (`packages/desktop/scripts/update-test-guards.ts`). The test hands it its feed.
pub const UPDATE_TEST: Identity = Identity {
    name: "updatetest",
    app_id: "com.tsnyders.tjommie.updatetest",
    product: "Tjommie Update Test",
    exe: "Tjommie Update Test.exe",
    data_root: DataRoot::Disposable,
    ports: Ports { app: 0, lan: 0 },
    update_feed: None,
    login_item: false,
};

/// The e2e tests' app: a temp root, any free port.
pub const E2E: Identity = Identity {
    name: "e2e",
    app_id: "com.tsnyders.tjommie.e2e",
    product: "Tjommie E2E",
    exe: "tjommie-desktop.exe",
    data_root: DataRoot::Disposable,
    ports: Ports { app: 0, lan: 0 },
    update_feed: None,
    login_item: false,
};

#[cfg(tjommie_identity = "production")]
pub const CURRENT: Identity = PRODUCTION;
#[cfg(tjommie_identity = "dev")]
pub const CURRENT: Identity = DEV;
#[cfg(tjommie_identity = "updatetest")]
pub const CURRENT: Identity = UPDATE_TEST;
#[cfg(tjommie_identity = "e2e")]
pub const CURRENT: Identity = E2E;

impl Identity {
    /// Where this run keeps its data: TJOMMIE_HOME when set (as in Electron's `dataRootFor`), else the identity's folder
    /// under %LOCALAPPDATA%. A disposable identity has no folder of its own and needs TJOMMIE_HOME. The caller still
    /// checks the result with `owner_safety::assert_root_allowed`.
    pub fn data_root(&self) -> Result<PathBuf, String> {
        if let Some(home) = std::env::var_os("TJOMMIE_HOME") {
            return Ok(PathBuf::from(home));
        }
        match self.data_root {
            DataRoot::LocalAppData(folder) => std::env::var_os("LOCALAPPDATA")
                .map(|local| PathBuf::from(local).join(folder))
                .ok_or_else(|| "LOCALAPPDATA is not set".to_owned()),
            DataRoot::Disposable => Err(format!(
                "{} needs TJOMMIE_HOME, a folder under %TEMP%",
                self.product
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_has_no_feed() {
        // Dev and the test identities: no update feed and no start at login (spec §4); production has both.
        for identity in [DEV, UPDATE_TEST, E2E] {
            assert_eq!(identity.update_feed, None, "{}", identity.name);
            assert!(!identity.login_item, "{}", identity.name);
        }
        assert_ne!(PRODUCTION.update_feed, None);
    }

    #[test]
    fn follows_tjommie_identity_and_defaults_to_dev() {
        assert_eq!(
            CURRENT.name,
            option_env!("TJOMMIE_IDENTITY").unwrap_or("dev")
        );
    }
}
