//! The real sites, by hand only (they are not asked by `cargo test`):
//! whether khinsider, the Qobuz-DL instance and the hifi-api instances
//! answer as the sources expect today.
//!
//! `cargo test -p nikgob-sources --test live -- --ignored --nocapture`

use std::sync::Arc;

use nikgob_core::source::{Fetched, Source, Want};
use nikgob_sources::select::rank;

fn eve() -> Want {
    Want {
        artists: vec!["Eve".into()],
        title: Some("Underdog".into()),
        album: Some("Underdog".into()),
        duration_ms: Some(194_000),
        ..Want::default()
    }
}

#[tokio::test]
#[ignore = "asks downloads.khinsider.com"]
async fn khinsider_answers() {
    let kh = nikgob_sources::khinsider::Khinsider::new("");
    let want = Want {
        artists: vec!["Con Fetti Pesto".into()],
        title: Some("Area1-Action".into()),
        album: Some("Supraland Six Inches Under Original Soundtrack".into()),
        ..Want::default()
    };
    let offers = kh.search(&want).await.unwrap();
    println!("khinsider: {offers:#?}");
    assert!(!offers.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = kh.fetch(&offers[0], &want, dir.path(), Arc::new(|_, _| {})).await.unwrap() else { panic!() };
    println!("khinsider: {:?} {} bytes", files[0], std::fs::metadata(&files[0]).unwrap().len());
}

#[tokio::test]
#[ignore = "asks the public Qobuz-DL instance"]
async fn qobuz_answers() {
    let q = nikgob_sources::qobuz::Qobuz::new(&[]);
    let r = q.search(&eve()).await;
    match &r {
        Ok(offers) => {
            let ranked = rank(offers.clone(), &eve(), &[]);
            println!("qobuz: {} offers, {} match: {:?}", offers.len(), ranked.len(), ranked.first().map(|r| r.offer.detail()));
        }
        Err(e) => println!("qobuz: {e}"),
    }
}

#[tokio::test]
#[ignore = "asks public hifi-api instances"]
async fn tidal_answers() {
    let t = nikgob_sources::tidal::Tidal::new(&[]);
    let r = t.search(&eve()).await;
    match &r {
        Ok(offers) => {
            let ranked = rank(offers.clone(), &eve(), &[]);
            println!("tidal: {} offers, {} match: {:?}", offers.len(), ranked.len(), ranked.first().map(|r| r.offer.detail()));
        }
        Err(e) => println!("tidal: {e}"),
    }
}
