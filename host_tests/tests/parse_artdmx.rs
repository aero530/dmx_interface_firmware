//! Feeds a packet built by `tools/dmxsend.py` through the firmware's own
//! parser, so the bring-up sender and the node cannot drift apart silently.
//!
//! `artdmx.hex` is one ArtDmx frame for 1:2:3, sequence 0x2a, with slot `i`
//! holding `(i * 7) & 0xFF`. To regenerate after changing the sender:
//!
//! ```text
//! python -c "import importlib.util,io; \n//!   spec=importlib.util.spec_from_file_location('d','tools/dmxsend.py'); \n//!   m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m); \n//!   io.open('host_tests/tests/artdmx.hex','w').write( \n//!     m.artnet_packet(m.parse_universe('1:2:3'), 0x2a, \n//!       bytes((i*7)&0xFF for i in range(512))).hex())"
//! ```
use common::artnet::tiny_artnet::{from_slice, Art};

#[test]
fn dmxsend_artdmx_parses() {
    let hex = include_str!("artdmx.hex").trim();
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect();

    let Ok(Art::Dmx(dmx)) = from_slice(&bytes) else {
        panic!("firmware parser rejected the packet dmxsend.py built");
    };
    assert_eq!(dmx.port_address.net, 1, "net");
    assert_eq!(dmx.port_address.sub_net, 2, "sub_net");
    assert_eq!(dmx.port_address.universe, 3, "universe");
    assert_eq!(dmx.sequence, 0x2a, "sequence");
    assert_eq!(dmx.physical, 0, "physical");
    assert_eq!(dmx.data.len(), 512, "slot count");
    let want: Vec<u8> = (0..512u32).map(|i| ((i * 7) & 0xFF) as u8).collect();
    assert_eq!(dmx.data, &want[..], "slot data round-tripped");
}
