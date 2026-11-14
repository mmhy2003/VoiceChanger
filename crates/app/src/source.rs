//! Which voice a session sends to the cable: the user's own or the model's.
//!
//! The original voice is only sent when the user chose it (the Model list entry or Bypass).
//! A model that is loading, failed or lost never falls back to it.

/// What a session sends to the cable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceSource {
    /// The microphone, unconverted.
    Original,
    /// The voice model's conversion.
    Model,
}

/// What the user wants sent: the original voice when no model is chosen or Bypass is on.
pub fn wanted_source(model_chosen: bool, bypass: bool) -> VoiceSource {
    if model_chosen && !bypass {
        VoiceSource::Model
    } else {
        VoiceSource::Original
    }
}

/// What Start runs; `None` when a model is wanted but not ready, which blocks Start.
pub fn start_source(wanted: VoiceSource, model_ready: bool) -> Option<VoiceSource> {
    match wanted {
        VoiceSource::Original => Some(VoiceSource::Original),
        VoiceSource::Model => model_ready.then_some(VoiceSource::Model),
    }
}

/// Whether a running session sends the wrong source and the wanted one can start.
pub fn source_restart(
    running: Option<VoiceSource>,
    wanted: VoiceSource,
    model_ready: bool,
) -> bool {
    running.is_some_and(|running| running != wanted && start_source(wanted, model_ready).is_some())
}

/// The source a running session restarts with, if it restarts: the wanted one when the source
/// must switch, else the running one when the restart is `forced` (a device, block, context or
/// crossfade change).
pub fn restart_source(
    running: VoiceSource,
    wanted: VoiceSource,
    model_ready: bool,
    forced: bool,
) -> Option<VoiceSource> {
    if source_restart(Some(running), wanted, model_ready) {
        Some(wanted)
    } else {
        forced.then_some(running)
    }
}

#[cfg(test)]
mod tests {
    use super::VoiceSource::{Model, Original};
    use super::*;

    #[test]
    fn wanted_source_table() {
        // (model_chosen, bypass)
        assert_eq!(wanted_source(false, false), Original);
        assert_eq!(wanted_source(false, true), Original);
        assert_eq!(wanted_source(true, true), Original);
        assert_eq!(wanted_source(true, false), Model);
    }

    #[test]
    fn start_source_never_falls_back_to_the_original_voice() {
        assert_eq!(start_source(Original, false), Some(Original));
        assert_eq!(start_source(Original, true), Some(Original));
        assert_eq!(start_source(Model, true), Some(Model));
        // A model that is loading, failed or lost blocks Start.
        assert_eq!(start_source(Model, false), None);
    }

    #[test]
    fn source_restart_table() {
        // Stopped: nothing to restart.
        for wanted in [Original, Model] {
            for ready in [false, true] {
                assert!(!source_restart(None, wanted, ready));
            }
        }
        // Already sending what is wanted.
        assert!(!source_restart(Some(Original), Original, false));
        assert!(!source_restart(Some(Original), Original, true));
        assert!(!source_restart(Some(Model), Model, false));
        // Going to the original voice is always possible. The session holds the model then,
        // so the slot is not "ready".
        assert!(source_restart(Some(Model), Original, false));
        // A model that is not ready never restarts the session.
        assert!(!source_restart(Some(Original), Model, false));
        assert!(source_restart(Some(Original), Model, true));
    }

    #[test]
    fn restart_source_table() {
        // (running, wanted, model_ready, forced)
        assert_eq!(restart_source(Original, Original, false, false), None);
        assert_eq!(restart_source(Model, Model, false, false), None);
        assert_eq!(
            restart_source(Model, Original, false, false),
            Some(Original)
        );
        assert_eq!(restart_source(Original, Model, true, false), Some(Model));
        // The chosen model is still loading: no restart on its own.
        assert_eq!(restart_source(Original, Model, false, false), None);
        // A device or block-size change restarts what was running.
        assert_eq!(restart_source(Original, Model, false, true), Some(Original));
        assert_eq!(restart_source(Model, Model, false, true), Some(Model));
        assert_eq!(
            restart_source(Original, Original, true, true),
            Some(Original)
        );
        // When the source is due to switch as well, the wanted one starts.
        assert_eq!(restart_source(Model, Original, false, true), Some(Original));
        assert_eq!(restart_source(Original, Model, true, true), Some(Model));
    }
}
