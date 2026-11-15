//! The help texts, one table per topic. A tip is an entry's `short`; the guide adds `more`.
//! Ranges and defaults are never typed here: `range_text` builds them from the code.

use super::{Id, Item};

const fn item(id: Id, name: &'static str, short: &'static str, more: &'static str) -> Item {
    Item {
        id: Some(id),
        name,
        short,
        more,
    }
}

const fn text(name: &'static str, short: &'static str, more: &'static str) -> Item {
    Item {
        id: None,
        name,
        short,
        more,
    }
}

pub static START: &[Item] = &[
    text(
        "1. Get a virtual cable",
        "VoiceChanger plays your voice into a virtual audio cable, such as the free VB-CABLE. Other apps then use that cable as their microphone.",
        "When no cable is found, Settings says so and shows a download link. You can still pick any output, such as your speakers, to try the app.",
    ),
    text(
        "2. Pick your devices",
        "In Settings, pick your Microphone, and as Virtual mic the cable's playback side, for example CABLE Input. Cables are marked as recommended.",
        "Turn on Monitor to hear yourself in your headphones.",
    ),
    text(
        "3. Add a voice",
        "On the Models tab, press Import… or drop an RVC v2 .onnx file on the window.",
        "A PyTorch .pth or .zip model is converted into the library on the Convert tab first.",
    ),
    text(
        "4. Press Start",
        "On the Voice tab, pick the voice in the Model list and press Start.",
        "Original voice (no model) sends your microphone unchanged and needs no model. Closing the window keeps the voice running in the system tray.",
    ),
    text(
        "5. Use it in other apps",
        "In Discord, OBS, Zoom or a game, choose the cable's recording side, for example CABLE Output, as the microphone.",
        "Do not pick that recording side as VoiceChanger's own Microphone: it would send the voice back into the cable.",
    ),
    text(
        "6. Fine-tune",
        "Hover any control to see what it does, with its range and default. Press F1 or ? Help to open this guide, or the ? next to a heading for that part.",
        "Troubleshooting lists what to change when something sounds wrong.",
    ),
];

pub static VOICE: &[Item] = &[
    item(
        Id::Model,
        "Model",
        "The voice you send. Original voice (no model) sends your microphone unchanged; any other entry converts your voice with that model.",
        "Picking another voice while running switches to it once it has loaded. Only models whose file exists are listed; add more on the Models tab.",
    ),
    item(
        Id::StartStop,
        "Start / Stop",
        "Starts or stops sending audio to the virtual mic. When Start is greyed out, the line below it says why.",
        "Start needs a microphone, a cable output and, for a voice model, a loaded model. The Start/Stop hotkey and the tray menu do the same.",
    ),
    item(
        Id::Bypass,
        "Bypass",
        "Sends your original voice while the chosen voice model stays loaded, so you can switch back without loading it again.",
        "The audio restarts underneath, so there is a short gap. Bypass is never saved: it is off each time VoiceChanger starts. The tray menu can flip it too.",
    ),
    item(
        Id::Pitch,
        "Pitch",
        "Moves the converted voice up or down in semitones; 12 semitones is one octave.",
        "Raise it when the voice sounds too low for the model; lower it when it sounds too high. The value is remembered for each model; a model you have not used yet starts on its Default pitch from the Models tab. Applies to converted voices only.",
    ),
    item(
        Id::InputGain,
        "Input gain",
        "Makes your microphone louder or quieter before anything else. The Input meter shows the level after this gain.",
        "Raise it when the Input meter barely moves while you speak; lower it when it reaches 0 dB, which distorts the voice.",
    ),
    item(
        Id::OutputGain,
        "Output gain",
        "Makes the voice sent to the virtual mic louder or quieter. The monitor follows it too.",
        "Raise it when other apps hear you too quietly; lower it when the Output meter reaches 0 dB.",
    ),
    item(
        Id::NoiseGate,
        "Noise gate",
        "Silences sound quieter than this level, so background noise between words is not converted.",
        "Raise it (towards -20 dB) when noise between words comes through; lower it when quiet words or word endings are cut off. Applies to converted voices only.",
    ),
    item(
        Id::Meters,
        "Input and Output meters",
        "Show how loud your microphone (after Input gain) and the sent voice are while running.",
        "The bars cover -60 to 0 dB. Speak normally: the Input bar should move clearly and stay below 0 dB.",
    ),
    item(
        Id::Latency,
        "Latency",
        "The delay from your microphone to the virtual mic, measured while running. It turns orange above 300 ms.",
        "Block and Crossfade add to it. Lower Block to reduce it, as long as Inference does not turn orange.",
    ),
    item(
        Id::Inference,
        "Inference",
        "How long the model took for the last block, against the block length. It turns orange above 60% of a block.",
        "When it is orange, raise Block, lower Extra context or pick a faster provider; otherwise blocks fall behind and the voice breaks up.",
    ),
    item(
        Id::LateBlocks,
        "Late blocks",
        "How many blocks of your voice were skipped because conversion fell behind.",
        "If it keeps rising, raise Block, lower Extra context or pick a faster provider in Settings > Performance.",
    ),
    item(
        Id::Underruns,
        "Underruns",
        "How often the virtual mic ran out of audio and got silence instead; each one is a short gap.",
        "If it keeps rising, follow the advice for Late blocks.",
    ),
];

pub static MODELS: &[Item] = &[
    item(
        Id::Import,
        "Import…",
        "Adds RVC v2 .onnx voice models to the library. You can also drop files on the window.",
        "Each model is checked and copied into the library. When a model does not say its sample rate, pick 32, 40 or 48 kHz. A .pth or .zip file is offered to the Convert tab instead.",
    ),
    item(
        Id::OpenFolder,
        "Open folder",
        "Opens the folder where the library keeps the model files and their icons.",
        "",
    ),
    item(
        Id::Reorder,
        "Move up and down",
        "Moves a model up or down in the list. The Voice tab's Model list uses the same order.",
        "",
    ),
    item(
        Id::ModelName,
        "Model name",
        "Click a model's name to use it; the highlighted one is in use.",
        "A model whose file is missing is greyed out and marked as file missing.",
    ),
    item(
        Id::DefaultPitch,
        "Default pitch",
        "The pitch a model starts on the first time you use it. After that the Voice tab remembers your own pitch for it.",
        "Drag it or type a value; a drag is saved when you let go.",
    ),
    item(
        Id::IconRenameDelete,
        "Icon…, Rename, Delete",
        "Icon… gives the model a picture (PNG or JPEG), Rename changes its name, and Delete removes the model and its file.",
        "Delete asks first. The model in use can only be deleted while the voice is stopped.",
    ),
];

pub static CONVERT: &[Item] = &[
    item(
        Id::ChooseFile,
        "Choose file…",
        "Opens a PyTorch voice model (.pth, or a .zip holding one) to convert. You can also drop it on this tab.",
        "The file is read first, and its version, sample rate, pitch support and speakers are shown.",
    ),
    item(
        Id::ConvertForm,
        "Name, Default pitch and Icon",
        "How the new model appears in your library: its name, the pitch it starts on and an optional picture.",
        "You can change all three later on the Models tab.",
    ),
    item(
        Id::ConvertButton,
        "Convert",
        "Converts the file into an ONNX model and adds it to your library. It needs a name.",
        "Converting can take a while; Cancel stops it.",
    ),
    item(
        Id::AfterConvert,
        "After converting",
        "Use now picks the new model, Show in Models opens the library, Export .onnx… saves a copy of the file, and Convert another… starts again.",
        "",
    ),
];

pub static DEVICES: &[Item] = &[
    item(
        Id::Microphone,
        "Microphone",
        "The microphone VoiceChanger listens to. While running, a level bar shows next to it.",
        "Do not pick a cable's recording side (for example CABLE Output) here: it would send the voice back into the cable.",
    ),
    item(
        Id::VirtualMic,
        "Virtual mic",
        "Where the voice is sent: the playback side of your virtual cable, for example CABLE Input. Cables are marked as recommended.",
        "Other apps then pick the cable's recording side (CABLE Output) as their microphone. Changing a device while running restarts the audio.",
    ),
    item(
        Id::Monitor,
        "Monitor",
        "Plays the sent voice to a second output, such as your headphones, so you hear yourself.",
        "Do not pick the virtual mic's device here: everything listening to the cable would hear the monitor too.",
    ),
    item(
        Id::MonitorVolume,
        "Volume",
        "How loud you hear yourself on the monitor. It does not change what other apps hear.",
        "",
    ),
    item(
        Id::Refresh,
        "Refresh",
        "Lists the audio devices again, for example after plugging in a headset or installing a cable.",
        "A chosen device that is gone is replaced by the Windows default, with a note.",
    ),
];

pub static PERFORMANCE: &[Item] = &[
    item(
        Id::Provider,
        "Provider",
        "Where the voice model runs: DirectML on most graphics cards, CUDA on NVIDIA cards, or CPU. It can only be changed while stopped.",
        "CUDA needs a CUDA build of onnxruntime.dll. If a provider cannot start, the model runs on the CPU instead; the status bar shows which one is used.",
    ),
    item(
        Id::Gpu,
        "GPU",
        "Which graphics card DirectML or CUDA uses, counted from 0. Only shown when the provider is not CPU.",
        "Change it when the computer has more than one graphics card and the slower one is used.",
    ),
    item(
        Id::Block,
        "Block",
        "How much audio is converted at a time. Smaller blocks mean less delay; larger blocks give a slow graphics card more time.",
        "Lower it to reduce Latency while Inference is not orange; raise it when Inference turns orange or late blocks rise. Changing it while running restarts the audio.",
    ),
    item(
        Id::ExtraContext,
        "Extra context",
        "How much of the audio before each block the model also hears. More context gives the model more to go on but takes more inference time.",
        "Lower it when Inference turns orange. Changing it while running restarts the audio.",
    ),
    item(
        Id::Crossfade,
        "Crossfade",
        "How long each block blends into the next, to hide the joins. Its length is added to the delay.",
        "Raise it when you hear clicks between blocks; lower it to cut delay. Changing it while running restarts the audio.",
    ),
];

pub static GENERAL: &[Item] = &[
    item(
        Id::Hotkey,
        "Hotkey",
        "A key combination that starts and stops the voice from anywhere, also while VoiceChanger is in the tray. Off by default.",
        "Press Set…, then the keys. It needs Ctrl, Alt or Shift; Esc cancels. Clear turns it off.",
    ),
    item(
        Id::StartInTray,
        "Start in the tray",
        "Starts VoiceChanger with only its tray icon, without opening the window.",
        "Click the tray icon to open the window. The voice never starts by itself.",
    ),
    item(
        Id::Updates,
        "Updates",
        "Checks GitHub for a new VoiceChanger every 6 hours, and can download it in the background. Check for updates asks now.",
        "For each new version you choose Update now, Later or Skip this version. Models and settings are kept.",
    ),
    item(
        Id::OpenLogs,
        "Open logs folder",
        "Opens the folder with VoiceChanger's daily log files. Attach the latest one when you report a problem.",
        "",
    ),
    item(
        Id::About,
        "About…",
        "Shows the version, the licence, the repository and where VoiceChanger keeps its models, settings and logs.",
        "",
    ),
];

pub static TROUBLESHOOTING: &[Item] = &[
    text(
        "Other apps do not hear me",
        "Check that Virtual mic is the cable's playback side (CABLE Input), that the other app uses its recording side (CABLE Output) as microphone, and that VoiceChanger is running.",
        "Watch the Output meter: when it moves, VoiceChanger is sending.",
    ),
    text(
        "The voice breaks up or stutters",
        "Look at Inference in the status bar. When it is orange or late blocks rise, raise Block, lower Extra context or pick a faster provider.",
        "",
    ),
    text(
        "The voice comes too late",
        "Lower Block, and then Crossfade, a little at a time, while watching that Inference does not turn orange.",
        "Latency in the status bar shows the delay; it turns orange above 300 ms.",
    ),
    text(
        "The voice sounds too high or too low",
        "Move Pitch on the Voice tab. 12 semitones is one octave.",
        "Pitch is remembered for each model.",
    ),
    text(
        "Background noise comes through",
        "Raise Noise gate until the noise between words is silent, but not so far that quiet words are cut.",
        "",
    ),
    text(
        "I hear myself twice",
        "Turn Monitor off, or pick headphones for it instead of the cable's device.",
        "",
    ),
    text(
        "Start is greyed out",
        "The line under Start says why: usually a missing microphone or cable output in Settings, or a voice model that is still loading.",
        "Press Refresh in Settings after plugging a device in.",
    ),
    text(
        "A model is marked file missing",
        "Its file was moved or deleted from the models folder. Delete the entry and import the model again.",
        "",
    ),
];
