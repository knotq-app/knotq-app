//! Media-variant scenarios (i).

use super::super::{Harness, D0, D1};

// ---------------------------------------------------------------------------
// Scenario i — Media variants
// ---------------------------------------------------------------------------

/// Two devices attach DIFFERENT images to the same scheme concurrently; both assets
/// must survive after sync.  (In-memory only for the oversized-upload check; the HTTP
/// variant skips that part since it goes through the real R2 path which is also gated.)
pub fn scenario_i_media_variants(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Media Scheme", &["item0", "item1"]);
    h.settle();

    // Attach different images on D0 and D1 concurrently.
    let img_a: Vec<u8> = (0u32..1024).map(|i| (i % 251) as u8).collect();
    let img_b: Vec<u8> = (0u32..2048).map(|i| (i % 127) as u8).collect();

    let (_, name_a) = h.attach_image_to_device(D0, s, 0, img_a.clone());
    let (_, name_b) = h.attach_image_to_device(D1, s, 1, img_b.clone());

    h.sync(D0);
    let remote_latest_d0 = h.device_remote_latest(D0);
    h.upload_media(D0, &remote_latest_d0).expect("upload A");

    h.sync(D1);
    let remote_latest_d1 = h.device_remote_latest(D1);
    h.upload_media(D1, &remote_latest_d1).expect("upload B");

    h.sync(D0);
    h.download_media(D0);
    h.sync(D1);
    h.download_media(D1);

    h.settle();
    h.assert_all_converged();

    // Both assets must be present on both devices.
    assert!(
        h.device(D0).media_assets.contains_key(&name_a),
        "D0 missing its own asset"
    );
    assert!(
        h.device(D0).media_assets.contains_key(&name_b),
        "D0 missing D1's asset"
    );
    assert!(
        h.device(D1).media_assets.contains_key(&name_a),
        "D1 missing D0's asset"
    );
    assert!(
        h.device(D1).media_assets.contains_key(&name_b),
        "D1 missing its own asset"
    );

    // Asset bytes must be intact.
    assert_eq!(
        h.device(D0).media_assets[&name_a],
        img_a,
        "A image bytes corrupted"
    );
    assert_eq!(
        h.device(D1).media_assets[&name_b],
        img_b,
        "B image bytes corrupted"
    );
}

/// Image attached then scheme deleted — the other device must tolerate the orphan
/// content doc that lingers server-side.
pub fn scenario_i_media_scheme_deleted(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Doomed Media Scheme", &["item"]);
    h.settle();

    let img: Vec<u8> = (0u32..512).map(|i| (i % 251) as u8).collect();
    let (_, _name) = h.attach_image_to_device(D0, s, 0, img.clone());
    h.sync(D0);
    let remote = h.device_remote_latest(D0);
    h.upload_media(D0, &remote).expect("upload");

    // D0 deletes the scheme.
    h.archive_scheme(D0, s);
    h.delete_scheme(D0, s);
    h.sync(D0);

    // D1 syncs — must not error even though the content doc lingers.
    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_absent(D1, s);
}
