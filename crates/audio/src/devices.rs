//! Output devices by any of their names.
//!
//! The list the settings show is fastframe-audio's: each device's display
//! name, which is also the name it is opened by. On Linux that is the
//! PulseAudio description of a sink ("fifine Headset Analog Stereo"). A host
//! without a screen names its sink the way PulseAudio does
//! (`alsa_output.usb-3142_fifine_Headset-00.analog-stereo`, what `pactl
//! list sinks` prints as `Name:`), so a name that is no display name is
//! looked up among the devices' ids (cpal's PulseAudio host gives the sink's
//! name as the id) and turned into the display name of that device.

/// An output device as the system lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    /// The name shown, and the one it is opened by.
    pub name: String,
    /// The system's own id of it (PulseAudio: the sink's name), where the
    /// host gives one.
    pub id: Option<String>,
}

/// Every output device with its id. Nothing is opened; it asks the sound
/// server, so it is not for the audio callback or the player's loop.
pub fn output_devices() -> Result<Vec<OutputDevice>, String> {
    #[cfg(target_os = "linux")]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let devices = host.output_devices().map_err(|e| e.to_string())?;
        Ok(devices
            .map(|d| OutputDevice {
                name: d.to_string(),
                id: d.id().ok().map(|id| id.id().to_owned()),
            })
            .collect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        fastframe_audio::output_device_names()
            .map(|names| names.into_iter().map(|name| OutputDevice { name, id: None }).collect())
            .map_err(|e| e.to_string())
    }
}

/// The display name of the device `wanted` names: its own when a device is
/// shown by that name, else that of the device whose id it is (a PulseAudio
/// sink's name, also with cpal's `host:` before it). None when no device
/// has that name or id now (it may come back; the default output plays
/// meanwhile).
pub fn resolve_device(wanted: &str, devices: &[OutputDevice]) -> Option<String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    if let Some(d) = devices.iter().find(|d| d.name == wanted) {
        return Some(d.name.clone());
    }
    let by_id = |id: &str| devices.iter().find(|d| d.id.as_deref() == Some(id)).map(|d| d.name.clone());
    by_id(wanted).or_else(|| wanted.split_once(':').and_then(|(_, id)| by_id(id.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi() -> Vec<OutputDevice> {
        vec![
            OutputDevice {
                name: "Built-in Audio Digital Stereo (HDMI)".into(),
                id: Some("alsa_output.platform-107c701400.hdmi.hdmi-stereo".into()),
            },
            OutputDevice {
                name: "fifine Headset Analog Stereo".into(),
                id: Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo".into()),
            },
            OutputDevice { name: "Speakers (USB DAC)".into(), id: None },
        ]
    }

    #[test]
    fn a_display_name_stays_itself() {
        assert_eq!(
            resolve_device("fifine Headset Analog Stereo", &pi()).as_deref(),
            Some("fifine Headset Analog Stereo")
        );
        assert_eq!(resolve_device(" Speakers (USB DAC) ", &pi()).as_deref(), Some("Speakers (USB DAC)"));
    }

    #[test]
    fn a_sink_name_selects_its_sink() {
        assert_eq!(
            resolve_device("alsa_output.usb-3142_fifine_Headset-00.analog-stereo", &pi()).as_deref(),
            Some("fifine Headset Analog Stereo")
        );
        // cpal's id as it prints it, host first.
        assert_eq!(
            resolve_device("pulseaudio:alsa_output.usb-3142_fifine_Headset-00.analog-stereo", &pi()).as_deref(),
            Some("fifine Headset Analog Stereo")
        );
    }

    #[test]
    fn a_name_no_device_has_resolves_to_nothing() {
        assert_eq!(resolve_device("alsa_output.usb-other-00.analog-stereo", &pi()), None);
        assert_eq!(resolve_device("", &pi()), None);
        assert_eq!(resolve_device("   ", &pi()), None);
        assert_eq!(resolve_device("fifine", &pi()), None, "no guessing by a part of a name");
        assert_eq!(resolve_device("fifine Headset Analog Stereo", &[]), None);
    }

    #[test]
    fn a_display_name_wins_over_an_id() {
        // A device shown by a name that is another device's id: the name.
        let mut list = pi();
        list.push(OutputDevice {
            name: "alsa_output.usb-3142_fifine_Headset-00.analog-stereo".into(),
            id: Some("other".into()),
        });
        assert_eq!(
            resolve_device("alsa_output.usb-3142_fifine_Headset-00.analog-stereo", &list).as_deref(),
            Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo")
        );
    }
}
