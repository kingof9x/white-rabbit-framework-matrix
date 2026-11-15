//! The Windows sound devices, through WASAPI in shared mode, as Python's `soundcard` package uses it:
//! 48 kHz float stereo with AUTOCONVERTPCM | SRC_DEFAULT_QUALITY, so Windows converts whatever the
//! device's own format is. Playback is the default output device's loopback stream.

use std::time::Duration;

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

use super::{Backend, Device, CHUNK_SIZE, SAMPLE_RATE};

const CHANNELS: u16 = 2;
/// WAVE_FORMAT_EXTENSIBLE, SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT and KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
/// written out to avoid pulling in two large `windows` feature modules for three constants.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const STEREO_MASK: u32 = 0x3;
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// WASAPI's buffer, in 100 ns units (200 ms).
const BUFFER_DURATION: i64 = 2_000_000;
/// How long to wait between looks at the buffer (a packet is usually 10 ms).
const POLL: Duration = Duration::from_millis(5);

pub(crate) struct Wasapi;

/// COM on this thread, multithreaded. A thread that already has COM another way (e.g. the UI
/// thread, single-threaded) keeps it, and COM works there too.
struct Com {
    initialised: bool,
}

impl Com {
    fn init() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com { initialised: hr.is_ok() }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { CoUninitialize() };
        }
    }
}

fn text(e: windows::core::Error) -> String {
    e.to_string()
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// Copies a string COM allocated, and frees it.
unsafe fn take_string(value: PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    text
}

fn describe(device: &IMMDevice) -> windows::core::Result<Device> {
    unsafe {
        let id = take_string(device.GetId()?);
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let name = PropVariantToStringAlloc(&value).map(|s| take_string(s));
        let _ = PropVariantClear(&mut value);
        Ok(Device { id, name: name? })
    }
}

impl Backend for Wasapi {
    fn check_available(&self) -> Result<(), String> {
        let _com = Com::init();
        enumerator().map(|_| ()).map_err(|e| format!("Windows audio isn't available: {e}"))
    }

    fn default_speaker(&self) -> Result<Device, String> {
        let _com = Com::init();
        unsafe { describe(&enumerator().map_err(text)?.GetDefaultAudioEndpoint(eRender, eConsole).map_err(text)?) }
            .map_err(text)
    }

    fn microphones(&self) -> Result<Vec<Device>, String> {
        let _com = Com::init();
        let list = || -> windows::core::Result<Vec<Device>> {
            unsafe {
                let devices = enumerator()?.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
                (0..devices.GetCount()?).map(|i| describe(&devices.Item(i)?)).collect()
            }
        };
        list().map_err(text)
    }

    fn default_microphone(&self) -> Result<Device, String> {
        let _com = Com::init();
        unsafe { describe(&enumerator().map_err(text)?.GetDefaultAudioEndpoint(eCapture, eConsole).map_err(text)?) }
            .map_err(text)
    }

    fn record(
        &self,
        device: &Device,
        loopback: bool,
        keep_going: &dyn Fn() -> bool,
        sink: &mut dyn FnMut(&[f32], usize),
    ) -> Result<(), String> {
        let _com = Com::init();
        record(device, loopback, keep_going, sink).map_err(text)
    }
}

fn record(
    device: &Device,
    loopback: bool,
    keep_going: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(&[f32], usize),
) -> windows::core::Result<()> {
    let block_align = CHANNELS * 4;
    let format = WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: STEREO_MASK,
        SubFormat: SUBTYPE_IEEE_FLOAT,
    };
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    unsafe {
        let endpoint = enumerator()?.GetDevice(&HSTRING::from(device.id.as_str()))?;
        let client: IAudioClient = endpoint.Activate(CLSCTX_ALL, None)?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_DURATION,
            0,
            &format as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let result = read_loop(&capture, keep_going, sink);
        let _ = client.Stop();
        result
    }
}

/// Polls the capture buffer, handing on CHUNK_SIZE frames at a time. Loopback gives no packets
/// while nothing plays; the caller's staleness check turns that into silence.
fn read_loop(
    capture: &IAudioCaptureClient,
    keep_going: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(&[f32], usize),
) -> windows::core::Result<()> {
    let channels = CHANNELS as usize;
    let mut chunk: Vec<f32> = Vec::with_capacity(CHUNK_SIZE * channels * 2);
    while keep_going() {
        while unsafe { capture.GetNextPacketSize()? } > 0 {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut buffer_flags = 0u32;
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, None)? };
            let samples = frames as usize * channels;
            if buffer_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                chunk.resize(chunk.len() + samples, 0.0);
            } else {
                // The shared-mode buffer holds `frames` frames of the float format asked for above
                chunk.extend_from_slice(unsafe { std::slice::from_raw_parts(data as *const f32, samples) });
            }
            unsafe { capture.ReleaseBuffer(frames)? };
            if chunk.len() >= CHUNK_SIZE * channels {
                sink(&chunk, channels);
                chunk.clear();
            }
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}
