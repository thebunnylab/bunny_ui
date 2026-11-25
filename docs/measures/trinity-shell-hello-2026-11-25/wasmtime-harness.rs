//! The hello block on Trinity's Pure recipe (foundation/wasm-sandbox at
//! origin/dev): componentized from the hand-lowered core module, compiled
//! on the fueled engine, instantiated with the empty linker, called.
use std::time::Instant;
use wasmtime::component::Linker;

const WIT: &str = include_str!(concat!(env!("SHELL_WIT")));
const FUEL: u64 = 10_000_000_000;
const MEMORY: usize = 64 << 20;

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
    out.push(kind);
    out.extend_from_slice(payload);
    out
}
fn offer() -> Vec<u8> {
    let mut p = b"BNYS".to_vec();
    p.extend(1u32.to_le_bytes()); p.extend(1u32.to_le_bytes()); p.extend(0x3fu64.to_le_bytes());
    p.extend(320f64.to_le_bytes()); p.extend(140f64.to_le_bytes()); p.extend(2f64.to_le_bytes()); p.push(0);
    frame(0x40, &p)
}
fn pointer(phase: u8, x: f64, y: f64) -> Vec<u8> {
    let mut p = vec![phase]; p.extend(x.to_le_bytes()); p.extend(y.to_le_bytes()); p.extend([0, 1, 0]);
    frame(0x42, &p)
}
fn ms(t: Instant) -> f64 { t.elapsed().as_secs_f64() * 1000.0 }

fn main() {
    let path = std::env::args().nth(1).expect("block.wasm");
    let runs: usize = std::env::args().nth(2).map_or(5, |n| n.parse().unwrap());
    let core = std::fs::read(&path).unwrap();
    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve.push_str("shell.wit", WIT).unwrap();
    let world = resolve.select_world(&[pkg], Some("block")).unwrap();
    let mut module = core.clone();
    wit_component::embed_component_metadata(&mut module, &resolve, world, wit_component::StringEncoding::UTF8).unwrap();
    let t = Instant::now();
    let component_bytes = wasm_sandbox::build::componentize(&module).expect("componentizes");
    let componentize = ms(t);
    let engine = wasm_sandbox::fueled_engine().unwrap();
    let mut rows = Vec::new();
    for run in 0..runs {
        let t = Instant::now();
        let component = wasm_sandbox::fueled_component(&component_bytes).expect("compiles");
        let compile = ms(t);
        let imports = wasm_sandbox::imports(engine, &component);
        assert!(imports.is_empty(), "imports: {imports:?}");
        let serialized = component.serialize().map(|b| b.len()).unwrap_or(0);
        let mut store = wasm_sandbox::fueled_store(FUEL, MEMORY).unwrap();
        let linker: Linker<wasm_sandbox::Metered> = Linker::new(engine);
        let t = Instant::now();
        let instance = linker.instantiate(&mut store, &component).expect("instantiates");
        let instantiate = ms(t);
        let fuel0 = store.get_fuel().unwrap();
        let call = instance.get_typed_func::<(Vec<u8>,), (Vec<u8>,)>(&mut store, "call").unwrap();
        let t = Instant::now();
        let (first,) = call.call(&mut store, (offer(),)).expect("the handshake");
        call.post_return(&mut store).unwrap();
        let handshake = ms(t);
        let fuel1 = store.get_fuel().unwrap();
        // the button sits where the native layout puts it; read it off the semantics would
        // need the protocol here — the click point the node harness found
        let (x, y) = (std::env::var("PRESS_X").map_or(60.0, |v| v.parse().unwrap()), std::env::var("PRESS_Y").map_or(80.0, |v| v.parse().unwrap()));
        let mut press = pointer(1, x, y); press.extend(pointer(2, x, y));
        let t = Instant::now();
        let (second,) = call.call(&mut store, (press,)).expect("the press");
        call.post_return(&mut store).unwrap();
        let pressed = ms(t);
        let fuel2 = store.get_fuel().unwrap();
        if run == 0 {
            println!("component_bytes={} componentize_ms={componentize:.2} compiled_artifact_bytes={serialized}", component_bytes.len());
            println!("handshake_answer_bytes={} press_answer_bytes={}", first.len(), second.len());
            if let Ok(dir) = std::env::var("ANSWERS") {
                std::fs::write(format!("{dir}/wasmtime-handshake.bin"), &first).unwrap();
                std::fs::write(format!("{dir}/wasmtime-press.bin"), &second).unwrap();
            }
        }
        rows.push((compile, instantiate, handshake, pressed, fuel0 - fuel1, fuel1 - fuel2));
        println!("run {run}: compile_ms={compile:.1} instantiate_ms={instantiate:.3} handshake_ms={handshake:.3} press_ms={pressed:.3} fuel_handshake={} fuel_press={}", fuel0 - fuel1, fuel1 - fuel2);
    }
}
