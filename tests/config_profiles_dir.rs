//! `sbxm config profiles-dir` prints the folder sbxm reads profiles from, so
//! scripts such as `deploy-profiles.ps1` never re-implement the config parser
//! (decision 136). It loads only the global config: no profile, project, state
//! or `sbx`.

mod common;

use assert_cmd::Command;
use common::Env;
use sbxm::commands::config_profiles_dir;

fn sbxm() -> Command {
    Command::cargo_bin("sbxm").unwrap()
}

fn config_with(env: &Env, text: &str) {
    std::fs::write(env.config_dir().join("config.toml"), text).unwrap();
}

#[test]
fn prints_the_profiles_dir_the_config_names() {
    let env = Env::new();

    let out = config_profiles_dir::render(&env.config_dir()).unwrap();

    assert_eq!(out, format!("{}\n", env.profiles_dir().display()));
}

#[test]
fn defaults_to_profiles_in_the_config_dir() {
    let env = Env::new();
    config_with(
        &env,
        "base_dir = 'base'\n\n[resources]\ncpus = 4\nmemory = '8g'\n",
    );

    let out = config_profiles_dir::render(&env.config_dir()).unwrap();

    assert_eq!(
        out,
        format!("{}\n", env.config_dir().join("profiles").display())
    );
}

#[test]
fn reads_toml_the_way_sbxm_does_escapes_and_quoted_keys_included() {
    let env = Env::new();
    config_with(
        &env,
        "\"base_dir\" = 'base'\n\"profiles\\u005fdir\" = \"/srv/my\\\\profiles\"\n\n\
         [resources]\ncpus = 4\nmemory = '8g'\n",
    );

    let out = config_profiles_dir::render(&env.config_dir()).unwrap();

    assert_eq!(out, "/srv/my\\profiles\n");
}

#[test]
fn needs_no_profiles_projects_or_sbx() {
    let env = Env::new();
    std::fs::remove_dir_all(env.profiles_dir()).unwrap();

    let out = config_profiles_dir::render(&env.config_dir()).unwrap();

    assert_eq!(out, format!("{}\n", env.profiles_dir().display()));
}

#[test]
fn an_empty_profiles_dir_prints_as_the_current_directory() {
    let env = Env::new();
    config_with(
        &env,
        "base_dir = 'base'\nprofiles_dir = ''\n\n[resources]\ncpus = 4\nmemory = '8g'\n",
    );

    let out = config_profiles_dir::render(&env.config_dir()).unwrap();

    // sbxm joins profile names onto "", which is the current directory.
    assert_eq!(out, ".\n");
}

#[test]
fn a_config_sbxm_refuses_is_refused_with_its_message() {
    let env = Env::new();
    config_with(
        &env,
        "base_dir = 'base'\nprofiles_dri = 'x'\n\n[resources]\ncpus = 4\nmemory = '8g'\n",
    );

    let err = config_profiles_dir::render(&env.config_dir()).unwrap_err();

    assert!(
        format!("{err:#}").contains("unknown field `profiles_dri`"),
        "{err:#}"
    );
}

#[test]
fn an_unknown_table_is_refused_too() {
    let env = Env::new();
    config_with(
        &env,
        "base_dir = 'base'\n\n[resources]\ncpus = 4\nmemory = '8g'\n\n[profiles_dri]\nvalue = 'x'\n",
    );

    let err = config_profiles_dir::render(&env.config_dir()).unwrap_err();

    assert!(format!("{err:#}").contains("profiles_dri"), "{err:#}");
}

#[test]
fn no_config_points_to_config_init() {
    let tmp = tempfile::TempDir::new().unwrap();

    let err = config_profiles_dir::render(tmp.path()).unwrap_err();

    assert!(err.to_string().contains("sbxm config init"), "{err:#}");
}

#[test]
fn the_command_prints_only_the_path_on_stdout() {
    let env = Env::new();

    let out = sbxm()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["config", "profiles-dir"])
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}\n", env.profiles_dir().display())
    );
    assert!(out.stderr.is_empty());
}

#[test]
fn a_refused_config_prints_nothing_on_stdout_and_exits_non_zero() {
    let env = Env::new();
    config_with(&env, "base_dir = 'base'\nprofiles_dri = 'x'\n");

    let out = sbxm()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["config", "profiles-dir"])
        .assert()
        .failure()
        .get_output()
        .clone();

    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("profiles_dri"));
}
