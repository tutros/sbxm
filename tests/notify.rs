//! Issue 530 (decision 311): the notifier behind the loop's toasts, and the file of the keys
//! shown. The loop's own rules of when are in `task_loop_notify.rs`.

use std::cell::RefCell;
use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use sbxm::notify::{
    Notification, Notifier, Shown, TOAST_SCRIPT, Toast, XML_VAR, escape_xml, file_url, shown_path,
    toast_xml,
};

fn note(title: &str, body: &str) -> Notification {
    Notification {
        key: "k".into(),
        title: title.into(),
        body: body.into(),
        click: "https://github.com/o/r/pull/526?a=1&b=2".into(),
    }
}

#[test]
fn escape_xml_escapes_every_special_character() {
    assert_eq!(
        escape_xml(r#"<a & "b" 'c'>"#),
        "&lt;a &amp; &quot;b&quot; &apos;c&apos;&gt;"
    );
}

#[test]
fn the_toast_xml_escapes_title_body_and_click() {
    let xml = toast_xml(&note("PR <#1> & \"x\"", "it's <b>"));

    assert!(
        xml.contains("<text>PR &lt;#1&gt; &amp; &quot;x&quot;</text>"),
        "{xml}"
    );
    assert!(xml.contains("<text>it&apos;s &lt;b&gt;</text>"), "{xml}");
    assert!(
        xml.contains(
            "activationType=\"protocol\" launch=\"https://github.com/o/r/pull/526?a=1&amp;b=2\""
        ),
        "{xml}"
    );
    // Only the markup's own brackets are left.
    let text = xml.replace("<text>", "").replace("</text>", "");
    assert!(!text.contains("<b>"), "{xml}");
}

#[test]
fn on_windows_the_toast_runs_powershell_with_the_xml_out_of_the_command_line() {
    let calls = RefCell::new(Vec::new());
    let ps = |script: &str, xml: &str| -> Result<()> {
        calls.borrow_mut().push((script.to_owned(), xml.to_owned()));
        Ok(())
    };
    let toast = Toast {
        windows: true,
        powershell: &ps,
    };
    let mut out = Vec::new();

    toast.show(&note("a & b", "c"), &mut out).unwrap();

    let calls = calls.into_inner();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, TOAST_SCRIPT);
    assert!(TOAST_SCRIPT.contains(&format!("$env:{XML_VAR}")));
    assert!(!TOAST_SCRIPT.contains("a & b"));
    assert_eq!(calls[0].1, toast_xml(&note("a & b", "c")));
    assert!(out.is_empty());
}

#[test]
fn elsewhere_the_toast_writes_a_log_line_and_runs_no_powershell() {
    let ps = |_: &str, _: &str| -> Result<()> { panic!("no powershell.exe off Windows") };
    let toast = Toast {
        windows: false,
        powershell: &ps,
    };
    let mut out = Vec::new();

    toast
        .show(&note("Job run #529 refused", "why"), &mut out)
        .unwrap();

    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("Job run #529 refused: why"), "{out}");
}

#[test]
fn a_failing_powershell_is_an_error() {
    let ps = |_: &str, _: &str| -> Result<()> { bail!("no WinRT") };
    let toast = Toast {
        windows: true,
        powershell: &ps,
    };

    let e = toast.show(&note("t", "b"), &mut Vec::new()).unwrap_err();

    assert!(format!("{e:#}").contains("no WinRT"));
}

#[test]
fn a_file_url_of_a_windows_and_a_unix_path() {
    let windows = format!("I:{}sbxm{}log.md", char::from(92), char::from(92));
    assert_eq!(file_url(Path::new(&windows)), "file:///I:/sbxm/log.md");
    assert_eq!(file_url(Path::new("/tmp/log.md")), "file:///tmp/log.md");
}

#[test]
fn shown_keys_are_written_and_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mut warn = Vec::new();
    let mut shown = Shown::load(tmp.path(), &mut warn).unwrap();
    assert!(!shown.contains("pr:5:abc"));
    assert!(!shown_path(tmp.path()).exists());

    shown.record("pr:5:abc").unwrap();

    let again = Shown::load(tmp.path(), &mut warn).unwrap();
    assert!(again.contains("pr:5:abc"));
    assert!(warn.is_empty());
}

#[test]
fn an_unreadable_shown_file_is_a_warning_and_read_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let path = shown_path(tmp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "not json").unwrap();
    let mut warn = Vec::new();

    let mut shown = Shown::load(tmp.path(), &mut warn).unwrap();

    let warn = String::from_utf8(warn).unwrap();
    assert!(warn.contains("notified.json"), "{warn}");
    assert!(!shown.contains("x"));
    // Recording a key rewrites it whole.
    shown.record("x").unwrap();
    assert!(
        Shown::load(tmp.path(), &mut Vec::new())
            .unwrap()
            .contains("x")
    );
}
