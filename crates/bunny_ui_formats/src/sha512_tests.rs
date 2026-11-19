use super::*;
use crate::sha::hex;

#[test]
fn the_standard_vectors() {
    assert_eq!(
        hex(&sha512(b"")),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
         47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    assert_eq!(
        hex(&sha512(b"abc")),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(
        hex(&sha512(
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
        )),
        "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018\
         501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
    );
}

#[test]
fn a_million_as_in_odd_pieces() {
    let mut hash = Sha512::new();
    let piece = vec![b'a'; 997];
    let mut left = 1_000_000;
    while left > 0 {
        let take = left.min(piece.len());
        hash.update(&piece[..take]);
        left -= take;
    }
    assert_eq!(
        hex(&hash.finish()),
        "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973eb\
         de0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
    );
}

#[test]
fn every_length_round_a_block_edge_hashes_the_same_in_pieces() {
    // 111, 112 and 127 bytes are where the length moves to a second block
    let data: Vec<u8> = (0..300u32).map(|i| (i * 31 % 251) as u8).collect();
    for len in [0, 1, 111, 112, 113, 127, 128, 129, 239, 240, 255, 256, 300] {
        let whole = sha512(&data[..len]);
        let mut pieces = Sha512::new();
        for byte in &data[..len] {
            pieces.update(std::slice::from_ref(byte));
        }
        assert_eq!(pieces.finish(), whole, "length {len}");
    }
}
