//! Runs the fake-lossless check on files given on the command line:
//! `cargo run -p nikgob-import --example spectrum_check -- a.flac b.flac`.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use nikgob_core::quality::{Mp3Target, cutoff_text, label, verdict_text};
use nikgob_import::quality::{CheckError, check_file};
use nikgob_import::tags::probe_info;

fn main() {
    let stop = AtomicBool::new(false);
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        let info = match probe_info(path) {
            Ok(i) => i,
            Err(e) => {
                println!("{arg}: {e}");
                continue;
            }
        };
        match check_file(path, &stop) {
            Ok(c) => {
                println!("{arg}");
                println!(
                    "  {} {:?} kbps, {:?} Hz, {:?} bit, {:?} ch, {:.1} s",
                    info.codec,
                    info.bitrate_kbps,
                    info.sample_rate,
                    info.bit_depth,
                    info.channels,
                    info.duration.as_secs_f64()
                );
                println!(
                    "  verdict {:?}, cutoff {:.0} Hz, steepness {:.1} dB, origin {}",
                    c.verdict, c.cutoff_hz, c.steepness_db, c.origin
                );
                println!("  label «{}»", label(&info.codec, info.bitrate_kbps, Some(&c.summary())));
                println!("  «{}»", verdict_text(&info.codec, info.bitrate_kbps, &c));
                println!("  band {}", cutoff_text(&c).unwrap_or_else(|| "?".into()));
                println!("  would convert to {:?}", Mp3Target::for_origin(c.origin));
            }
            Err(CheckError::Stopped) => println!("{arg}: stopped"),
            Err(CheckError::Unavailable(e) | CheckError::Undecodable(e)) => println!("{arg}: {e}"),
        }
    }
}
