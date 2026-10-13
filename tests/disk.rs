//! Issue 210 (decision 194): the free-space check of the drive holding `sbx`'s sandbox state,
//! its two `config.toml` keys and the refusal every sandbox-creating command shares.

mod common;

use std::path::{Path, PathBuf};

use common::{Env, toml_path};
use sbxm::config::GlobalConfig;
use sbxm::disk::{self, Guard};

const GIB: u64 = 1024 * 1024 * 1024;

fn guard(free_space: fn(&Path) -> std::io::Result<u64>) -> Guard {
    Guard {
        free_space,
        ..Guard::default()
    }
}

#[test]
fn sizes_parse_with_a_unit_or_as_zero() {
    assert_eq!(disk::parse_size("10g").unwrap(), 10 * GIB);
    assert_eq!(disk::parse_size("10G").unwrap(), 10 * GIB);
    assert_eq!(disk::parse_size("512m").unwrap(), 512 * 1024 * 1024);
    assert_eq!(disk::parse_size("1t").unwrap(), 1024 * GIB);
    assert_eq!(disk::parse_size("64k").unwrap(), 64 * 1024);
    assert_eq!(disk::parse_size("0").unwrap(), 0);
    for bad in [
        "",
        "g",
        "ten",
        "10x",
        "10 g",
        "-1g",
        "10",
        "99999999999999999999g",
    ] {
        assert!(disk::parse_size(bad).is_err(), "{bad:?} parsed");
    }
}

#[test]
fn the_threshold_defaults_to_10_gib() {
    let env = Env::new();
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.min_free_disk, 10 * GIB);
    assert_eq!(config.sandbox_state_dir, None);
}

#[test]
fn both_keys_are_read_from_config_toml() {
    let env = Env::new();
    let state = env.tmp.path().join("state");
    env.set_config(&format!(
        "min_free_disk = \"0\"\nsandbox_state_dir = {}",
        toml_path(&state)
    ));
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.min_free_disk, 0);
    assert_eq!(config.sandbox_state_dir, Some(state));
}

#[test]
fn a_bad_threshold_is_refused_naming_the_key_and_the_file() {
    let env = Env::new();
    env.set_config("min_free_disk = \"lots\"");
    let err = format!("{:#}", GlobalConfig::load(&env.config_dir()).unwrap_err());
    let path = env.config_dir().join("config.toml");
    assert_eq!(
        err,
        format!(
            "min_free_disk \"lots\" in {} isn't a size; use a number with a unit, like \"10g\" \
             (k, m, g or t), or \"0\" to switch the check off",
            path.display()
        )
    );
}

/// A state folder that exists, with the threshold left at its default.
fn env_with_state() -> (Env, PathBuf) {
    let env = Env::new();
    let state = env.tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    env.set_config(&format!("sandbox_state_dir = {}", toml_path(&state)));
    (env, state)
}

fn require(env: &Env, guard: &Guard) -> anyhow::Result<()> {
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    disk::require(&config, &env.config_dir(), guard)
}

#[test]
fn under_the_threshold_refuses_naming_the_drive_the_free_space_and_the_flag() {
    let (env, state) = env_with_state();

    let err = require(&env, &guard(|_| Ok(5 * GIB / 2))).unwrap_err();

    assert_eq!(
        format!("{err:#}"),
        format!(
            "only 2.5 GiB free on {} (sbx's sandbox state), under min_free_disk \
             10.0 GiB; free up space, or pass --ignore-disk to go ahead anyway (min_free_disk \
             in {} sets the threshold, \"0\" switches the check off)",
            sbxm::disk::describe(&state),
            env.config_dir().join("config.toml").display()
        )
    );
}

#[test]
fn at_or_over_the_threshold_goes_ahead() {
    let (env, _) = env_with_state();
    require(&env, &guard(|_| Ok(10 * GIB))).unwrap();
}

#[test]
fn ignore_disk_goes_ahead_without_reading_the_drive() {
    let (env, _) = env_with_state();
    let guard = Guard {
        ignore: true,
        ..guard(|_| panic!("free space read despite --ignore-disk"))
    };
    require(&env, &guard).unwrap();
}

#[test]
fn a_zero_threshold_switches_the_check_off() {
    let (env, _) = env_with_state();
    env.set_config("min_free_disk = \"0\"");
    require(
        &env,
        &guard(|_| panic!("free space read with the check off")),
    )
    .unwrap();
}

#[test]
fn a_lower_threshold_lets_less_free_space_through() {
    let (env, _) = env_with_state();
    env.set_config("min_free_disk = \"2g\"");
    require(&env, &guard(|_| Ok(5 * GIB / 2))).unwrap();
}

#[test]
fn without_a_known_state_folder_the_check_is_skipped() {
    let env = Env::new();
    require(&env, &guard(|_| panic!("free space read without a folder"))).unwrap();
}

#[test]
fn the_platform_default_folder_is_checked_when_the_key_is_unset() {
    let env = Env::new();
    let state = env.tmp.path().join("default-state");
    std::fs::create_dir(&state).unwrap();
    let guard = Guard {
        default_state_dir: Some(state.clone()),
        ..guard(|_| Ok(GIB))
    };

    let err = format!("{:#}", require(&env, &guard).unwrap_err());

    assert!(err.contains(&state.display().to_string()), "{err}");
}

#[test]
fn the_key_overrides_the_platform_default() {
    let (env, state) = env_with_state();
    let guard = Guard {
        default_state_dir: Some(env.tmp.path().join("elsewhere")),
        ..guard(|_| Ok(GIB))
    };

    let err = format!("{:#}", require(&env, &guard).unwrap_err());

    assert!(err.contains(&state.display().to_string()), "{err}");
}

#[test]
fn a_missing_state_folder_goes_ahead() {
    let env = Env::new();
    env.set_config(&format!(
        "sandbox_state_dir = {}",
        toml_path(&env.tmp.path().join("missing"))
    ));
    require(
        &env,
        &guard(|_| panic!("free space read for a missing folder")),
    )
    .unwrap();
}

#[test]
fn an_unreadable_free_space_goes_ahead() {
    let (env, _) = env_with_state();
    require(&env, &guard(|_| Err(std::io::Error::other("no")))).unwrap();
}

// The other tests take the wording from `describe`, so its two forms are pinned here, each on
// the platform whose paths can produce it.
#[cfg(windows)]
#[test]
fn a_path_with_a_drive_is_described_by_its_drive_letter() {
    assert_eq!(
        disk::describe(Path::new(r"E:\sbx\state")),
        r"drive E: (E:\sbx\state)"
    );
}

#[cfg(not(windows))]
#[test]
fn a_path_without_a_drive_is_described_by_its_folder() {
    assert_eq!(
        disk::describe(Path::new("/var/sbx/state")),
        "the drive of /var/sbx/state"
    );
}
