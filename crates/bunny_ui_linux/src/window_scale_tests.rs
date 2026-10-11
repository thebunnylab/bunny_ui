//! Native wire regression for the 2 -> 1 buffer-scale transition.

use super::*;

#[test]
#[ignore = "Requires an isolated Wayland compositor; run by check-linux-accessibility.sh"]
fn buffer_scale_reset_reaches_the_wayland_wire() {
    const CHILD: &str = "BUNNY_SCALE_WIRE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        assert!(!is_x11());
        let handle = create_window(
            "Buffer scale protocol",
            200.0,
            100.0,
            WindowOptions {
                scene: false,
                resizable: true,
                minimizable: true,
                min: None,
            },
        );
        let window = handle.window;
        // These are controlled native protocol inputs, not simulated monitor
        // observations. The parent inspects actual libwayland request messages.
        for scale in [2, 1] {
            with_client(|client| window_at(client, window).unwrap().scale = scale);
            let (width, height) = (200 * scale, 100 * scale);
            present_rows(
                window,
                width,
                height,
                &vec![255; width * height * 4],
                &[(
                    0,
                    0,
                    i64::try_from(width).unwrap(),
                    i64::try_from(height).unwrap(),
                )],
            );
            let display = with_client(|client| client.display);
            assert!(unsafe { wl_display_roundtrip(display) } >= 0);
            absorb_setup(window);
        }
        // Exercise the shared GPU pre-present envelope too. This proves its
        // scale requests, not a GPU swap or a presentation outcome.
        assert!(gpu_pre_present(window, 2));
        assert!(gpu_pre_present(window, 1));
        let display = with_client(|client| client.display);
        assert!(unsafe { wl_display_roundtrip(display) } >= 0);
        assert_eq!(unsafe { wl_display_get_error(display) }, 0);
        close_top_level(window);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ffi::window_scale_tests::buffer_scale_reset_reaches_the_wayland_wire",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("WAYLAND_DEBUG", "client")
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native scale fixture failed: {log}"
    );
    let mut scales = std::collections::HashMap::<&str, Vec<i32>>::new();
    for line in log.lines() {
        let Some((prefix, argument)) = line.split_once(".set_buffer_scale(") else {
            continue;
        };
        let surface = prefix.split_whitespace().last().unwrap();
        let scale = argument.split(')').next().unwrap().parse().unwrap();
        scales.entry(surface).or_default().push(scale);
        println!("{line}");
    }
    assert!(
        scales.values().any(|values| values == &[2, 1, 2, 1]),
        "CPU commits and GPU envelopes must both reset the same surface to one: {log}"
    );
    println!(
        "Wayland wire observed buffer scales 2,1,2,1 on one native surface; mixed-DPI and GPU presentation remain unqualified"
    );
}
