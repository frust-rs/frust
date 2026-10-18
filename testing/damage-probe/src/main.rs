//! Desktop entry point: parse the flags, build the event loop, run the probe.

use damage_probe::{Config, DamageMode};

const USAGE: &str = "damage-probe — does this backend + compositor honour wgpu's damage hint?

USAGE: damage-probe [--damage none|exact|under] [--full-every N] [--step PX] [--size PX] [--fps N]

  --damage none    plain Queue::present (control)
  --damage exact   old + new square bounds via present_with_damage (default)
  --damage under   NEW bounds only — a compositor that honours the hint shows a trail
  --full-every N   report the whole surface every Nth frame (wipes the trail); 0 = never
  --step PX        pixels the square moves per frame (default 4)
  --size PX        square edge (default 48)
  --fps N          pace frames to N per second (default: swapchain / vsync bound)

Environment: WGPU_BACKEND=vulkan|gl|dx12|metal selects the backend (wgpu's own knob);
RUST_LOG=info shows adapter, surface and VK_KHR_incremental_present facts.
Press Escape or close the window to quit.";

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();

    let mut cfg = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .unwrap_or_else(|| exit_usage(&format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--damage" | "-d" => {
                let v = value("--damage");
                cfg.mode = DamageMode::parse(&v)
                    .unwrap_or_else(|| exit_usage(&format!("unknown damage mode {v:?}")));
            }
            "--full-every" => cfg.full_every = parse_u32(&value("--full-every"), "--full-every"),
            "--step" => cfg.step = parse_u32(&value("--step"), "--step").max(1),
            "--size" => cfg.size = parse_u32(&value("--size"), "--size").max(1),
            "--fps" => cfg.fps = Some(parse_u32(&value("--fps"), "--fps").max(1)),
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => exit_usage(&format!("unknown argument {other:?}")),
        }
    }

    let event_loop = match winit::event_loop::EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("event loop: {e}");
            std::process::exit(2);
        }
    };
    damage_probe::run(event_loop, cfg);
}

fn parse_u32(s: &str, name: &str) -> u32 {
    s.trim()
        .parse()
        .unwrap_or_else(|_| exit_usage(&format!("{name} wants an integer, got {s:?}")))
}

fn exit_usage(msg: &str) -> ! {
    eprintln!("damage-probe: {msg}\n\n{USAGE}");
    std::process::exit(2)
}
