//! WASAPI device listing, default selection and feedback warnings.

use cpal::traits::{DeviceTrait, HostTrait};

const CABLE_MARKERS: [&str; 3] = ["cable", "voicemeeter", "virtual audio cable"];

/// True for virtual audio cable endpoints (VB-CABLE, VoiceMeeter, Virtual Audio Cable).
pub fn is_virtual_cable(name: &str) -> bool {
    let lower = name.to_lowercase();
    CABLE_MARKERS.iter().any(|m| lower.contains(m))
}

/// An endpoint as WASAPI lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Endpoint ID (`cpal::DeviceId` as text); `""` if it could not be read.
    pub id: String,
    /// Friendly name.
    pub name: String,
}

/// A chosen device, matched by `id` first and then by `name`. Both empty = nothing selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRef {
    /// Endpoint ID; `""` if unknown.
    pub id: String,
    /// Friendly name, used for display and as the fallback match.
    pub name: String,
}

impl DeviceRef {
    /// True when nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.id.is_empty() && self.name.is_empty()
    }

    /// Same endpoint: by ID when both have one, else by non-empty name.
    pub fn same(&self, other: &DeviceRef) -> bool {
        if !self.id.is_empty() && !other.id.is_empty() {
            self.id == other.id
        } else {
            !self.name.is_empty() && self.name == other.name
        }
    }
}

impl From<&DeviceInfo> for DeviceRef {
    fn from(d: &DeviceInfo) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
        }
    }
}

/// Audio endpoints as enumerated by WASAPI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// Capture devices, in enumeration order.
    pub inputs: Vec<DeviceInfo>,
    /// Playback devices: virtual cables first (stable sort), otherwise in enumeration order.
    pub outputs: Vec<DeviceInfo>,
    /// The Windows default capture device, if any.
    pub default_input: Option<DeviceRef>,
    /// The Windows default playback device, if any.
    pub default_output: Option<DeviceRef>,
}

impl DeviceList {
    /// Builds a list, moving virtual cables to the front of `outputs`.
    pub fn new(
        inputs: Vec<DeviceInfo>,
        mut outputs: Vec<DeviceInfo>,
        default_input: Option<DeviceRef>,
        default_output: Option<DeviceRef>,
    ) -> Self {
        outputs.sort_by_key(|d| !is_virtual_cable(&d.name)); // stable: cables first
        Self {
            inputs,
            outputs,
            default_input,
            default_output,
        }
    }

    /// Builds a list from names alone, using each name as its ID.
    pub fn from_names(
        inputs: Vec<String>,
        outputs: Vec<String>,
        default_input: Option<String>,
        default_output: Option<String>,
    ) -> Self {
        let info = |name: String| DeviceInfo {
            id: name.clone(),
            name,
        };
        let device_ref = |name: String| DeviceRef {
            id: name.clone(),
            name,
        };
        Self::new(
            inputs.into_iter().map(info).collect(),
            outputs.into_iter().map(info).collect(),
            default_input.map(device_ref),
            default_output.map(device_ref),
        )
    }

    /// True if any output is a virtual cable.
    pub fn has_cable(&self) -> bool {
        self.outputs.iter().any(|d| is_virtual_cable(&d.name))
    }

    /// The input `r` refers to: by ID first, then by name.
    pub fn find_input(&self, r: &DeviceRef) -> Option<&DeviceInfo> {
        find_device(&self.inputs, r)
    }

    /// The output `r` refers to: by ID first, then by name.
    pub fn find_output(&self, r: &DeviceRef) -> Option<&DeviceInfo> {
        find_device(&self.outputs, r)
    }

    /// Windows default input if it is in `inputs`, else the first input.
    pub fn default_input_device(&self) -> Option<&DeviceInfo> {
        self.default_input
            .as_ref()
            .and_then(|d| self.find_input(d))
            .or_else(|| self.inputs.first())
    }

    /// First virtual cable output, else the Windows default output (if listed), else the first output.
    pub fn default_cable(&self) -> Option<&DeviceInfo> {
        self.outputs
            .iter()
            .find(|d| is_virtual_cable(&d.name))
            .or_else(|| {
                self.default_output
                    .as_ref()
                    .and_then(|d| self.find_output(d))
            })
            .or_else(|| self.outputs.first())
    }
}

fn find_device<'a>(list: &'a [DeviceInfo], r: &DeviceRef) -> Option<&'a DeviceInfo> {
    if r.is_empty() {
        return None;
    }
    list.iter()
        .find(|d| !r.id.is_empty() && d.id == r.id)
        .or_else(|| list.iter().find(|d| !r.name.is_empty() && d.name == r.name))
}

/// The devices the user picked in Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Capture device; empty means nothing selected.
    pub input: DeviceRef,
    /// Cable output; empty means nothing selected.
    pub cable: DeviceRef,
    /// Monitor output, if any.
    pub monitor: Option<DeviceRef>,
    /// Whether audio is also sent to the monitor.
    pub monitor_enabled: bool,
}

impl Selection {
    /// input = default_input_device, cable = default_cable,
    /// monitor = default_output if it is listed and differs from the cable, monitor_enabled = false.
    pub fn defaults(list: &DeviceList) -> Self {
        let input: DeviceRef = list
            .default_input_device()
            .map(Into::into)
            .unwrap_or_default();
        let cable: DeviceRef = list.default_cable().map(Into::into).unwrap_or_default();
        let monitor = list
            .default_output
            .as_ref()
            .and_then(|d| list.find_output(d))
            .map(DeviceRef::from)
            .filter(|m| !m.same(&cable));
        Self {
            input,
            cable,
            monitor,
            monitor_enabled: false,
        }
    }

    /// After a Refresh: a selected device that is still listed is refreshed silently (its ID and
    /// name are updated). A missing one falls back to its default.
    /// A missing monitor becomes `None` and monitor_enabled = false.
    /// An empty input or cable takes its default without a note.
    /// Returns one note per fallback, e.g.
    /// `Input "USB Mic" is no longer available; switched to "Microphone (Realtek)".`
    /// `Monitor "Headphones" is no longer available; monitor turned off.`
    /// If there is no default to fall back to, the field becomes empty and the note ends with `; no device selected.`
    pub fn reconcile(&mut self, list: &DeviceList) -> Vec<String> {
        let mut notes = Vec::new();
        let input = list.find_input(&self.input);
        reconcile_field(
            &mut self.input,
            input,
            list.default_input_device(),
            "Input",
            &mut notes,
        );
        let cable = list.find_output(&self.cable);
        reconcile_field(
            &mut self.cable,
            cable,
            list.default_cable(),
            "Cable output",
            &mut notes,
        );
        if let Some(monitor) = &self.monitor {
            match list.find_output(monitor) {
                Some(found) => self.monitor = Some(found.into()),
                None => {
                    notes.push(format!(
                        "Monitor \"{}\" is no longer available; monitor turned off.",
                        monitor.name
                    ));
                    self.monitor = None;
                    self.monitor_enabled = false;
                }
            }
        }
        notes
    }
}

fn reconcile_field(
    field: &mut DeviceRef,
    found: Option<&DeviceInfo>,
    default: Option<&DeviceInfo>,
    label: &str,
    notes: &mut Vec<String>,
) {
    if let Some(found) = found {
        *field = found.into();
        return;
    }
    if !field.is_empty() {
        let outcome = match default {
            Some(d) => format!("switched to \"{}\"", d.name),
            None => "no device selected".to_string(),
        };
        notes.push(format!(
            "{label} \"{}\" is no longer available; {outcome}.",
            field.name
        ));
    }
    *field = default.map(Into::into).unwrap_or_default();
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
/// MonitorSameAsCable: monitor_enabled && the monitor is the same device as the (non-empty) cable.
/// InputIsCableRecordingSide: the input name contains "cable output" (case-insensitive) or
/// is_virtual_cable(input name).
pub fn feedback_warnings(sel: &Selection) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if sel.monitor_enabled
        && !sel.cable.is_empty()
        && sel.monitor.as_ref().is_some_and(|m| m.same(&sel.cable))
    {
        warnings.push(Warning::MonitorSameAsCable);
    }
    let input = &sel.input.name;
    if input.to_lowercase().contains("cable output") || is_virtual_cable(input) {
        warnings.push(Warning::InputIsCableRecordingSide);
    }
    warnings
}

/// Why Start is disabled, or None. Checked in this order:
/// list.inputs empty   -> "No input devices found. Connect a microphone and press Refresh."
/// list.outputs empty  -> "No output devices found. Install VB-CABLE or connect speakers, then press Refresh."
/// sel.input not found in list.inputs  -> "Pick an input device in Settings."
/// sel.cable not found in list.outputs -> "Pick a cable output in Settings."
pub fn start_blocker(list: &DeviceList, sel: &Selection) -> Option<&'static str> {
    if list.inputs.is_empty() {
        Some("No input devices found. Connect a microphone and press Refresh.")
    } else if list.outputs.is_empty() {
        Some("No output devices found. Install VB-CABLE or connect speakers, then press Refresh.")
    } else if list.find_input(&sel.input).is_none() {
        Some("Pick an input device in Settings.")
    } else if list.find_output(&sel.cable).is_none() {
        Some("Pick a cable output in Settings.")
    } else {
        None
    }
}

/// Enumerates WASAPI endpoints with their IDs and names. Never fails: errors give empty lists.
pub fn list_devices() -> DeviceList {
    crate::com::keep_alive();
    let Ok(host) = cpal::host_from_id(cpal::HostId::Wasapi) else {
        return DeviceList::default();
    };
    let inputs = host
        .input_devices()
        .map(|ds| ds.filter_map(|d| device_info(&d)).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|ds| ds.filter_map(|d| device_info(&d)).collect())
        .unwrap_or_default();
    let default_input = host.default_input_device().and_then(|d| device_info(&d));
    let default_output = host.default_output_device().and_then(|d| device_info(&d));
    DeviceList::new(
        inputs,
        outputs,
        default_input.as_ref().map(Into::into),
        default_output.as_ref().map(Into::into),
    )
}

/// The device's ID (empty if it cannot be read) and name; `None` without a name.
fn device_info(d: &cpal::Device) -> Option<DeviceInfo> {
    let name = d.description().ok()?.name().to_string();
    let id = d.id().map(|i| i.to_string()).unwrap_or_default();
    Some(DeviceInfo { id, name })
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

    /// A ref whose ID is its name, as `from_names` builds them.
    fn r(name: &str) -> DeviceRef {
        dref(name, name)
    }

    fn dref(id: &str, name: &str) -> DeviceRef {
        DeviceRef {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    fn info(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    fn sel(input: &str, cable: &str, monitor: Option<&str>, monitor_enabled: bool) -> Selection {
        Selection {
            input: r(input),
            cable: r(cable),
            monitor: monitor.map(r),
            monitor_enabled,
        }
    }

    fn name(d: Option<&DeviceInfo>) -> Option<&str> {
        d.map(|d| d.name.as_str())
    }

    fn names(ds: &[DeviceInfo]) -> Vec<&str> {
        ds.iter().map(|d| d.name.as_str()).collect()
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
            names(&l.outputs),
            [CABLE_IN, "VoiceMeeter Input", "Speakers", "Headphones"]
        );
        assert!(l.has_cable());
        assert!(!list(&[], &[SPEAKERS], None, None).has_cable());
    }

    #[test]
    fn new_sorts_cables_first_by_name() {
        let l = DeviceList::new(
            vec![],
            vec![info("a", SPEAKERS), info("b", CABLE_IN)],
            None,
            None,
        );
        assert_eq!(l.outputs, [info("b", CABLE_IN), info("a", SPEAKERS)]);
    }

    #[test]
    fn device_ref_same_and_empty() {
        assert!(DeviceRef::default().is_empty());
        assert!(!dref("", "Mic").is_empty());
        assert!(!dref("x", "").is_empty());
        // Both have an ID: only the ID counts.
        assert!(dref("x", "A").same(&dref("x", "B")));
        assert!(!dref("x", "A").same(&dref("y", "A")));
        // Otherwise by non-empty name.
        assert!(dref("", "A").same(&dref("y", "A")));
        assert!(!DeviceRef::default().same(&DeviceRef::default()));
        assert_eq!(DeviceRef::from(&info("x", "A")), dref("x", "A"));
    }

    #[test]
    fn find_matches_id_then_name() {
        let l = DeviceList::new(
            vec![info("id-mic", MIC), info("id-usb", USB_MIC)],
            vec![info("id-spk", SPEAKERS), info("id-hp", HEADPHONES)],
            None,
            None,
        );
        // The ID wins over a changed (or another device's) name.
        assert_eq!(
            l.find_input(&dref("id-mic", USB_MIC)),
            Some(&info("id-mic", MIC))
        );
        // Unknown ID: matched by name.
        assert_eq!(
            l.find_input(&dref("gone", USB_MIC)),
            Some(&info("id-usb", USB_MIC))
        );
        assert_eq!(
            l.find_output(&dref("", HEADPHONES)),
            Some(&info("id-hp", HEADPHONES))
        );
        assert_eq!(l.find_input(&dref("gone", "Gone")), None);
        assert_eq!(l.find_input(&DeviceRef::default()), None);
        assert_eq!(l.find_output(&DeviceRef::default()), None);
    }

    #[test]
    fn default_cable_prefers_first_cable() {
        let l = list(&[], &[SPEAKERS, CABLE_IN], None, Some(SPEAKERS));
        assert_eq!(name(l.default_cable()), Some(CABLE_IN));
    }

    #[test]
    fn default_cable_falls_back_to_windows_default() {
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, Some(HEADPHONES));
        assert_eq!(name(l.default_cable()), Some(HEADPHONES));
    }

    #[test]
    fn default_cable_falls_back_to_first_output() {
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, Some("Gone"));
        assert_eq!(name(l.default_cable()), Some(SPEAKERS));
        let l = list(&[], &[SPEAKERS, HEADPHONES], None, None);
        assert_eq!(name(l.default_cable()), Some(SPEAKERS));
    }

    #[test]
    fn default_cable_none_when_no_outputs() {
        let l = list(&[MIC], &[], Some(MIC), Some(SPEAKERS));
        assert_eq!(l.default_cable(), None);
    }

    #[test]
    fn default_input_uses_windows_default_or_first() {
        assert_eq!(
            name(list(&[MIC, USB_MIC], &[], Some(USB_MIC), None).default_input_device()),
            Some(USB_MIC)
        );
        assert_eq!(
            name(list(&[MIC, USB_MIC], &[], Some("Gone"), None).default_input_device()),
            Some(MIC)
        );
        assert_eq!(
            name(list(&[MIC, USB_MIC], &[], None, None).default_input_device()),
            Some(MIC)
        );
        assert_eq!(list(&[], &[], Some(MIC), None).default_input_device(), None);
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

        // The default output is not listed: no monitor.
        let l = list(&[MIC], &[CABLE_IN], Some(MIC), Some(SPEAKERS));
        assert_eq!(Selection::defaults(&l), sel(MIC, CABLE_IN, None, false));

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
    fn reconcile_refreshes_renamed_device_silently() {
        let l = DeviceList::new(
            vec![info("id-mic", MIC)],
            vec![info("id-cable", CABLE_IN), info("id-hp", HEADPHONES)],
            None,
            None,
        );
        let mut s = Selection {
            input: dref("id-mic", "Old Mic Name"),
            cable: dref("id-cable", "Old Cable Name"),
            monitor: Some(dref("id-hp", "Old Headphones")),
            monitor_enabled: true,
        };
        assert!(s.reconcile(&l).is_empty());
        assert_eq!(
            s,
            Selection {
                input: dref("id-mic", MIC),
                cable: dref("id-cable", CABLE_IN),
                monitor: Some(dref("id-hp", HEADPHONES)),
                monitor_enabled: true,
            }
        );
    }

    #[test]
    fn reconcile_by_name_when_id_changed() {
        let l = DeviceList::new(
            vec![info("id-mic", MIC), info("new-usb", USB_MIC)],
            vec![info("new-cable", CABLE_IN)],
            Some(dref("id-mic", MIC)),
            None,
        );
        let mut s = Selection {
            input: dref("old-usb", USB_MIC),
            cable: dref("old-cable", CABLE_IN),
            monitor: None,
            monitor_enabled: false,
        };
        assert!(s.reconcile(&l).is_empty());
        assert_eq!(s.input, dref("new-usb", USB_MIC));
        assert_eq!(s.cable, dref("new-cable", CABLE_IN));
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
    fn monitor_same_as_cable_by_id() {
        let mut s = Selection {
            input: r(MIC),
            cable: dref("id-cable", CABLE_IN),
            monitor: Some(dref("id-cable", "Renamed Cable")),
            monitor_enabled: true,
        };
        assert_eq!(feedback_warnings(&s), [Warning::MonitorSameAsCable]);
        // Same name, different endpoints: not the same device.
        s.monitor = Some(dref("id-other", CABLE_IN));
        assert!(feedback_warnings(&s).is_empty());
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
        // A renamed device is still found by its ID.
        let renamed = Selection {
            input: dref(MIC, "Old Name"),
            ..valid.clone()
        };
        assert_eq!(start_blocker(&l, &renamed), None);
    }

    #[test]
    fn list_devices_does_not_panic() {
        let _ = list_devices();
    }
}
