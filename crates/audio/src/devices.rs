//! WASAPI device listing, default selection and feedback warnings.

use cpal::traits::{DeviceTrait, HostTrait};

const CABLE_MARKERS: [&str; 3] = ["cable", "voicemeeter", "virtual audio cable"];

/// True for virtual audio cable endpoints (VB-CABLE, VoiceMeeter, Virtual Audio Cable).
pub fn is_virtual_cable(name: &str) -> bool {
    let lower = name.to_lowercase();
    CABLE_MARKERS.iter().any(|m| lower.contains(m))
}

/// Audio endpoints by name, as enumerated by WASAPI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// Capture devices, in enumeration order.
    pub inputs: Vec<String>,
    /// Playback devices: virtual cables first (stable sort), otherwise in enumeration order.
    pub outputs: Vec<String>,
    /// The Windows default capture device, if any.
    pub default_input: Option<String>,
    /// The Windows default playback device, if any.
    pub default_output: Option<String>,
}

impl DeviceList {
    /// Builds a list, moving virtual cables to the front of `outputs`.
    pub fn from_names(
        inputs: Vec<String>,
        mut outputs: Vec<String>,
        default_input: Option<String>,
        default_output: Option<String>,
    ) -> Self {
        outputs.sort_by_key(|n| !is_virtual_cable(n)); // stable: cables first
        Self {
            inputs,
            outputs,
            default_input,
            default_output,
        }
    }

    /// True if any output is a virtual cable.
    pub fn has_cable(&self) -> bool {
        self.outputs.iter().any(|n| is_virtual_cable(n))
    }

    /// Windows default input if it is in `inputs`, else the first input.
    pub fn default_input_name(&self) -> Option<&str> {
        listed_default(&self.inputs, self.default_input.as_deref())
            .or_else(|| self.inputs.first().map(String::as_str))
    }

    /// First virtual cable output, else the Windows default output (if listed), else the first output.
    pub fn default_cable(&self) -> Option<&str> {
        self.outputs
            .iter()
            .map(String::as_str)
            .find(|n| is_virtual_cable(n))
            .or_else(|| listed_default(&self.outputs, self.default_output.as_deref()))
            .or_else(|| self.outputs.first().map(String::as_str))
    }
}

fn listed_default<'a>(names: &'a [String], default: Option<&str>) -> Option<&'a str> {
    let default = default?;
    names.iter().map(String::as_str).find(|n| *n == default)
}

/// The devices the user picked in Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Capture device name; `""` means nothing selected.
    pub input: String,
    /// Cable output name; `""` means nothing selected.
    pub cable: String,
    /// Monitor output name, if any.
    pub monitor: Option<String>,
    /// Whether audio is also sent to the monitor.
    pub monitor_enabled: bool,
}

impl Selection {
    /// input = default_input_name, cable = default_cable,
    /// monitor = default_output if it differs from the cable, monitor_enabled = false.
    pub fn defaults(list: &DeviceList) -> Self {
        let input = list.default_input_name().unwrap_or_default().to_string();
        let cable = list.default_cable().unwrap_or_default().to_string();
        let monitor = list.default_output.clone().filter(|m| *m != cable);
        Self {
            input,
            cable,
            monitor,
            monitor_enabled: false,
        }
    }

    /// After a Refresh: any selected name missing from `list` falls back to its default.
    /// A missing monitor becomes `None` and monitor_enabled = false.
    /// An empty input or cable takes its default without a note.
    /// Returns one note per fallback, e.g.
    /// `Input "USB Mic" is no longer available; switched to "Microphone (Realtek)".`
    /// `Monitor "Headphones" is no longer available; monitor turned off.`
    /// If there is no default to fall back to, the field becomes "" and the note ends with `; no device selected.`
    pub fn reconcile(&mut self, list: &DeviceList) -> Vec<String> {
        let mut notes = Vec::new();
        reconcile_field(
            &mut self.input,
            &list.inputs,
            list.default_input_name(),
            "Input",
            &mut notes,
        );
        reconcile_field(
            &mut self.cable,
            &list.outputs,
            list.default_cable(),
            "Cable output",
            &mut notes,
        );
        if let Some(monitor) = &self.monitor
            && !list.outputs.contains(monitor)
        {
            notes.push(format!(
                "Monitor \"{monitor}\" is no longer available; monitor turned off."
            ));
            self.monitor = None;
            self.monitor_enabled = false;
        }
        notes
    }
}

fn reconcile_field(
    field: &mut String,
    names: &[String],
    default: Option<&str>,
    label: &str,
    notes: &mut Vec<String>,
) {
    if names.contains(field) {
        return;
    }
    let fallback = default.unwrap_or_default();
    if !field.is_empty() {
        let outcome = match default {
            Some(d) => format!("switched to \"{d}\""),
            None => "no device selected".to_string(),
        };
        notes.push(format!(
            "{label} \"{field}\" is no longer available; {outcome}."
        ));
    }
    *field = fallback.to_string();
}

/// A routing that is allowed but likely to cause feedback or confusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The monitor plays into the same device as the cable output.
    MonitorSameAsCable,
    /// The input is the recording side of a virtual cable.
    InputIsCableRecordingSide,
}

impl Warning {
    /// The text shown in Settings.
    pub fn message(self) -> &'static str {
        match self {
            Warning::MonitorSameAsCable => {
                "The monitor and the cable output are the same device. Everything listening to the cable will hear the monitor too; pick headphones or speakers for the monitor."
            }
            Warning::InputIsCableRecordingSide => {
                "The input is a virtual cable's recording side (e.g. \"CABLE Output\"). Sending it back into the cable creates a feedback loop; pick your microphone instead."
            }
        }
    }
}

/// Advisory warnings for a selection, in display order.
///
/// MonitorSameAsCable: monitor_enabled && monitor == Some(cable) && !cable.is_empty().
/// InputIsCableRecordingSide: input contains "cable output" (case-insensitive) or is_virtual_cable(input).
pub fn feedback_warnings(sel: &Selection) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if sel.monitor_enabled && !sel.cable.is_empty() && sel.monitor.as_deref() == Some(&sel.cable) {
        warnings.push(Warning::MonitorSameAsCable);
    }
    if sel.input.to_lowercase().contains("cable output") || is_virtual_cable(&sel.input) {
        warnings.push(Warning::InputIsCableRecordingSide);
    }
    warnings
}

/// Why Start is disabled, or None. Checked in this order:
/// list.inputs empty   -> "No input devices found. Connect a microphone and press Refresh."
/// list.outputs empty  -> "No output devices found. Install VB-CABLE or connect speakers, then press Refresh."
/// sel.input not in list.inputs  -> "Pick an input device in Settings."
/// sel.cable not in list.outputs -> "Pick a cable output in Settings."
pub fn start_blocker(list: &DeviceList, sel: &Selection) -> Option<&'static str> {
    if list.inputs.is_empty() {
        Some("No input devices found. Connect a microphone and press Refresh.")
    } else if list.outputs.is_empty() {
        Some("No output devices found. Install VB-CABLE or connect speakers, then press Refresh.")
    } else if !list.inputs.contains(&sel.input) {
        Some("Pick an input device in Settings.")
    } else if !list.outputs.contains(&sel.cable) {
        Some("Pick a cable output in Settings.")
    } else {
        None
    }
}

/// Enumerates WASAPI endpoints by name. Never fails: errors give empty lists.
pub fn list_devices() -> DeviceList {
    let Ok(host) = cpal::host_from_id(cpal::HostId::Wasapi) else {
        return DeviceList::default();
    };
    let inputs = host
        .input_devices()
        .map(|ds| ds.filter_map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|ds| ds.filter_map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    let default_input = host.default_input_device().and_then(|d| device_name(&d));
    let default_output = host.default_output_device().and_then(|d| device_name(&d));
    DeviceList::from_names(inputs, outputs, default_input, default_output)
}

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CABLE_IN: &str = "CABLE Input (VB-Audio Virtual Cable)";
    const CABLE_OUT: &str = "CABLE Output (VB-Audio Virtual Cable)";
    const MIC: &str = "Microphone (Realtek(R) Audio)";
    const USB_MIC: &str = "USB Mic";
    const SPEAKERS: &str = "Speakers (Realtek(R) Audio)";
    const HEADPHONES: &str = "Headphones (USB Audio)";

    fn list(inputs: &[&str], outputs: &[&str], di: Option<&str>, dout: Option<&str>) -> DeviceList {
        DeviceList::from_names(
            inputs.iter().map(|s| s.to_string()).collect(),
            outputs.iter().map(|s| s.to_string()).collect(),
            di.map(str::to_string),
            dout.map(str::to_string),
        )
    }

    fn sel(input: &str, cable: &str, monitor: Option<&str>, monitor_enabled: bool) -> Selection {
        Selection {
            input: input.to_string(),
            cable: cable.to_string(),
            monitor: monitor.map(str::to_string),
            monitor_enabled,
        }
    }

    #[test]
    fn detects_virtual_cables() {
        assert!(is_virtual_cable(CABLE_IN));
        assert!(is_virtual_cable(CABLE_OUT));
        assert!(is_virtual_cable(
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"
        ));
        assert!(is_virtual_cable("Line 1 (Virtual Audio Cable)"));
        assert!(is_virtual_cable("cable input"));
        assert!(!is_virtual_cable(SPEAKERS));
        assert!(!is_virtual_cable(HEADPHONES));
    }

    #[test]
    fn outputs_sorted_cables_first_stable() {
        let l = list(
            &[],
            &["Speakers", CABLE_IN, "Headphones", "VoiceMeeter Input"],
            None,
            None,
        );
        assert_eq!(
            l.outputs,
            [CABLE_IN, "VoiceMeeter Input", "Speakers", "Headphones"]
        );
        assert!(l.has_cable());
        assert!(!list(&[], &[SPEAKERS], None, None).has_cable());
    }

    #[test]
    fn default_cable_prefers_first_cable() {
        let l = list(&[], &[SPEAKERS, CABLE_IN], None, Some(SPEAKERS));
        assert_eq!(l.default_cable(), Some(CABLE_IN));
    }

    #[test]
    fn default_cable_falls_back_to_windows_default() {
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, Some(HEADPHONES));
        assert_eq!(l.default_cable(), Some(HEADPHONES));
    }

    #[test]
    fn default_cable_falls_back_to_first_output() {
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, Some("Gone"));
        assert_eq!(l.default_cable(), Some(SPEAKERS));
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, None);
        assert_eq!(l.default_cable(), Some(SPEAKERS));
    }

    #[test]
    fn default_cable_none_when_no_outputs() {
        let l = list(&[MIC], &[], Some(MIC), Some(SPEAKERS));
        assert_eq!(l.default_cable(), None);
    }

    #[test]
    fn default_input_uses_windows_default_or_first() {
        assert_eq!(
            list(&[MIC, USB_MIC], &[], Some(USB_MIC), None).default_input_name(),
            Some(USB_MIC)
        );
        assert_eq!(
            list(&[MIC, USB_MIC], &[], Some("Gone"), None).default_input_name(),
            Some(MIC)
        );
        assert_eq!(
            list(&[MIC, USB_MIC], &[], None, None).default_input_name(),
            Some(MIC)
        );
        assert_eq!(list(&[], &[], Some(MIC), None).default_input_name(), None);
    }

    #[test]
    fn selection_defaults() {
        let l = list(&[MIC], &[SPEAKERS, CABLE_IN], Some(MIC), Some(SPEAKERS));
        assert_eq!(
            Selection::defaults(&l),
            sel(MIC, CABLE_IN, Some(SPEAKERS), false)
        );

        // The default output is the cable itself: no monitor.
        let l = list(&[MIC], &[SPEAKERS], Some(MIC), Some(SPEAKERS));
        assert_eq!(Selection::defaults(&l), sel(MIC, SPEAKERS, None, false));

        assert_eq!(
            Selection::defaults(&DeviceList::default()),
            sel("", "", None, false)
        );
    }

    #[test]
    fn reconcile_keeps_valid_selection() {
        let l = list(
            &[MIC, USB_MIC],
            &[CABLE_IN, SPEAKERS, HEADPHONES],
            Some(MIC),
            Some(SPEAKERS),
        );
        let mut s = sel(USB_MIC, CABLE_IN, Some(HEADPHONES), true);
        let before = s.clone();
        assert!(s.reconcile(&l).is_empty());
        assert_eq!(s, before);
    }

    #[test]
    fn reconcile_falls_back_input_and_cable() {
        let l = list(&[MIC], &[CABLE_IN, SPEAKERS], Some(MIC), Some(SPEAKERS));
        let mut s = sel(USB_MIC, "Line 1 (Virtual Audio Cable)", None, false);
        let notes = s.reconcile(&l);
        assert_eq!(s, sel(MIC, CABLE_IN, None, false));
        assert_eq!(
            notes,
            [
                format!("Input \"{USB_MIC}\" is no longer available; switched to \"{MIC}\"."),
                format!(
                    "Cable output \"Line 1 (Virtual Audio Cable)\" is no longer available; switched to \"{CABLE_IN}\"."
                ),
            ]
        );
    }

    #[test]
    fn reconcile_drops_missing_monitor() {
        let l = list(&[MIC], &[CABLE_IN, SPEAKERS], Some(MIC), Some(SPEAKERS));
        let mut s = sel(MIC, CABLE_IN, Some(HEADPHONES), true);
        let notes = s.reconcile(&l);
        assert_eq!(s, sel(MIC, CABLE_IN, None, false));
        assert_eq!(
            notes,
            [format!(
                "Monitor \"{HEADPHONES}\" is no longer available; monitor turned off."
            )]
        );
    }

    #[test]
    fn reconcile_without_default() {
        let mut s = sel(USB_MIC, CABLE_IN, None, false);
        let notes = s.reconcile(&DeviceList::default());
        assert_eq!(s, sel("", "", None, false));
        assert_eq!(
            notes,
            [
                format!("Input \"{USB_MIC}\" is no longer available; no device selected."),
                format!("Cable output \"{CABLE_IN}\" is no longer available; no device selected."),
            ]
        );
    }

    #[test]
    fn reconcile_fills_empty_fields_silently() {
        let l = list(&[MIC], &[CABLE_IN], Some(MIC), None);
        let mut s = sel("", "", None, false);
        assert!(s.reconcile(&l).is_empty());
        assert_eq!(s, sel(MIC, CABLE_IN, None, false));
    }

    #[test]
    fn feedback_warnings_cases() {
        use Warning::*;
        assert!(feedback_warnings(&sel(MIC, CABLE_IN, Some(HEADPHONES), true)).is_empty());
        assert_eq!(
            feedback_warnings(&sel(MIC, CABLE_IN, Some(CABLE_IN), true)),
            [MonitorSameAsCable]
        );
        assert!(feedback_warnings(&sel(MIC, CABLE_IN, Some(CABLE_IN), false)).is_empty());
        assert!(feedback_warnings(&sel(MIC, "", Some(""), true)).is_empty());
        assert_eq!(
            feedback_warnings(&sel(CABLE_OUT, CABLE_IN, None, false)),
            [InputIsCableRecordingSide]
        );
        assert_eq!(
            feedback_warnings(&sel("VoiceMeeter Output", CABLE_IN, None, false)),
            [InputIsCableRecordingSide]
        );
        assert_eq!(
            feedback_warnings(&sel(CABLE_OUT, CABLE_IN, Some(CABLE_IN), true)),
            [MonitorSameAsCable, InputIsCableRecordingSide]
        );
        assert!(MonitorSameAsCable.message().contains("same device"));
        assert!(
            InputIsCableRecordingSide
                .message()
                .contains("feedback loop")
        );
    }

    #[test]
    fn start_blocker_cases() {
        let valid = sel(MIC, CABLE_IN, None, false);
        assert_eq!(
            start_blocker(&list(&[], &[CABLE_IN], None, None), &valid),
            Some("No input devices found. Connect a microphone and press Refresh.")
        );
        assert_eq!(
            start_blocker(&list(&[MIC], &[], None, None), &valid),
            Some(
                "No output devices found. Install VB-CABLE or connect speakers, then press Refresh."
            )
        );
        let l = list(&[MIC], &[CABLE_IN], None, None);
        assert_eq!(
            start_blocker(&l, &sel(USB_MIC, CABLE_IN, None, false)),
            Some("Pick an input device in Settings.")
        );
        assert_eq!(
            start_blocker(&l, &sel(MIC, "", None, false)),
            Some("Pick a cable output in Settings.")
        );
        assert_eq!(start_blocker(&l, &valid), None);
    }

    #[test]
    fn list_devices_does_not_panic() {
        let _ = list_devices();
    }
}
