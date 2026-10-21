//! Adversarial tests (break-b): the outbox on disk.

use super::*;

/// Messages waiting for an away machine survive a quit by living in
/// outbox.json. One entry the app can't read (written by another version,
/// or hand-damaged) must not throw away every other waiting message: the
/// next save would overwrite the file and they'd be gone for good, without
/// a word.
#[test]
fn one_unreadable_entry_doesnt_lose_every_waiting_message() {
    let d = std::env::temp_dir().join(format!("dent-bb-outbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // SAFETY: only this test in the crate reads DENT_CLIENT_STATE.
    unsafe { std::env::set_var("DENT_CLIENT_STATE", &d) };
    let good = |id: &str| {
        format!(
            r#"{{"session":"ab12/s1","id":"{id}","text":"please deploy","attachments":[],"at":1,"held":false}}"#
        )
    };
    // The middle entry lacks its text.
    let file = format!(
        "[{},{},{}]",
        good("m1"),
        r#"{"session":"ab12/s1","id":"m2","at":2}"#,
        good("m3")
    );
    std::fs::write(d.join("outbox.json"), &file).unwrap();
    let got = load();
    let ids: Vec<_> = got.iter().map(|o| o.id.as_str()).collect();
    // The app then saves what it holds as soon as anything changes.
    save(&got);
    let after = std::fs::read_to_string(d.join("outbox.json")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&d);
    assert!(
        ids.contains(&"m1") && ids.contains(&"m3"),
        "readable messages were dropped: loaded {ids:?}"
    );
    assert!(after.contains("m1") && after.contains("m3"), "{after}");
}
