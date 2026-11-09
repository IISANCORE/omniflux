//! playback-spike — the throwaway workbench that answers ONE question first:
//! can we decode 1080p video and present frames within the latency and RAM
//! budget on a potato-class machine? (Legit spike, not product code: it gets
//! deleted or graduated — it does not accrete.)

fn main() {
    // The spike's job right now is only to prove the toolchain + wiring.
    println!(
        "playback-spike scaffold ok — engine v{}",
        omniflux_core::engine_version()
    );
}
