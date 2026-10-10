//! Smoke test against the real MusicBrainz. Never runs by default:
//! `NIKGOB_METADATA_CONTACT=you@example.com cargo test -p nikgob-metadata --test live -- --ignored`

use std::time::Duration;

use nikgob_metadata::musicbrainz::{MusicBrainz, RecordingQuery};
use nikgob_metadata::{Http, MetadataConfig};

#[tokio::test]
#[ignore = "talks to the real MusicBrainz"]
async fn musicbrainz_search_and_lookup() {
    let contact = std::env::var("NIKGOB_METADATA_CONTACT")
        .expect("set NIKGOB_METADATA_CONTACT to a real contact for the User-Agent");
    let http = Http::new(&MetadataConfig::new(contact)).unwrap();
    let mb = MusicBrainz::new(http).unwrap();

    let query = RecordingQuery::new("Imagine")
        .artist("John Lennon")
        .duration(Duration::from_secs(183))
        .limit(5);
    let found = mb.search_recordings(&query).await.unwrap();
    assert!(!found.is_empty(), "search returned nothing");
    let first = &found[0];
    assert!(first.title.to_lowercase().contains("imagine"));

    let rec = mb.lookup_recording(&first.id).await.unwrap();
    assert_eq!(rec.id, first.id);
    assert!(!rec.artist_credit.is_empty());
}
