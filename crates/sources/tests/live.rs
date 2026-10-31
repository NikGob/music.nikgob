//! The real sites, by hand only (they are not asked by `cargo test`):
//! whether khinsider, the Qobuz-DL instance and the hifi-api instances
//! answer as the sources expect today.
//!
//! `cargo test -p nikgob-sources --test live -- --ignored --nocapture`

use std::sync::Arc;

use nikgob_core::source::{Fetched, Source, Want};
use nikgob_sources::select::{pick, rank};

mod common;

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

/// The wishes the torrents had nothing for, asked of the web sources as
/// they are today: what each gives, what the policy would take.
///
/// `cargo test -p nikgob-sources --test live failing_wishes_now -- --ignored --nocapture`
#[tokio::test]
#[ignore = "asks khinsider, Qobuz-DL and hifi-api for 42 wishes (several minutes)"]
async fn failing_wishes_now() {
    let sources: Vec<Arc<dyn Source>> = vec![
        Arc::new(nikgob_sources::khinsider::Khinsider::new("")),
        Arc::new(nikgob_sources::qobuz::Qobuz::new(&[])),
        Arc::new(nikgob_sources::tidal::Tidal::new(&[])),
    ];
    let mut found = 0;
    for w in common::WISHES {
        // The runtime asks with the credit's names apart.
        let mut want = common::want_of(w);
        want.artists = w.1.split(", ").filter(|a| !a.starts_with("и ещё")).map(str::to_owned).collect();
        let asks = sources.iter().map(|s| {
            let (s, want) = (s.clone(), &want);
            async move { (s.name().to_owned(), s.search(want).await) }
        });
        let mut offers = Vec::new();
        let mut line = Vec::new();
        for (name, r) in futures_util::future::join_all(asks).await {
            match r {
                Ok(o) => {
                    line.push(format!("{name} {}", o.len()));
                    offers.extend(o);
                }
                Err(e) => line.push(format!("{name}: {e}")),
            }
        }
        let ranked = rank(offers, &want, &[]);
        let verdict = match pick(&ranked, None) {
            Some(i) => {
                found += 1;
                format!("TAKE {} · {}", ranked[i].offer.detail(), ranked[i].offer.title)
            }
            None => match ranked.first() {
                Some(r) => format!("ASK {} · {}", r.offer.detail(), r.offer.title),
                None => "NOTHING".to_owned(),
            },
        };
        println!("{} — {} | {} | {verdict}", w.1, w.2, line.join(", "));
    }
    println!("taken by itself: {found} of {}", common::WISHES.len());
}
