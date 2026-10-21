//! This device's identity on disk. Its key is what every machine pinned
//! when it paired: a new key silently made in its place unpairs this
//! device from all of them, so a key that's there but can't be read must
//! be an error, never a fresh identity written over it.

#[test]
fn a_device_key_that_cant_be_read_is_never_replaced() {
    let dir = std::env::temp_dir().join(format!("dent-store-break-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: the only test in this binary, so no other thread reads the
    // environment meanwhile.
    unsafe { std::env::set_var("DENT_CLIENT_STATE", &dir) };
    let path = dir.join("device.key");
    let original = dent_remote::store::device_keys().unwrap();

    // Damaged on disk (a bad sector, a sync tool's conflict marker): bytes
    // that aren't text at all.
    let mut damaged = original.to_hex().into_bytes();
    damaged[10] = 0xff;
    std::fs::write(&path, &damaged).unwrap();
    let got = dent_remote::store::device_keys();
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(
        on_disk,
        damaged,
        "the damaged key was overwritten with a new identity (got {:?})",
        got.as_ref().map(|k| k.id())
    );
    assert!(
        got.is_err(),
        "a new identity was handed out: {:?}",
        got.map(|k| k.id())
    );
    let _ = std::fs::remove_dir_all(&dir);
}
