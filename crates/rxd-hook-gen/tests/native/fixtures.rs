// Independently authored ELF64 little-endian fixture: two PT_LOAD mappings.
// Header at VMA 0x700000, text file offset 0x1000 at VMA 0x702000.
// Runtime module offset is 0x2000, NOT the file offset 0x1000.
// No compiled/executed sample is involved. Code is x86-64 `mov eax,edi; ret`.
fn u16le(b: &mut [u8], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn u32le(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn u64le(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
pub fn elf(stripped: bool) -> Vec<u8> {
    let mut b = vec![0; 0x1100];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    u16le(&mut b, 16, 3);
    u16le(&mut b, 18, 62);
    u32le(&mut b, 20, 1);
    u64le(&mut b, 32, 64);
    u64le(&mut b, 40, 0x400);
    u16le(&mut b, 52, 64);
    u16le(&mut b, 54, 56);
    u16le(&mut b, 56, 2);
    u16le(&mut b, 58, 64);
    u16le(&mut b, 60, 4);
    u16le(&mut b, 62, 3);
    for (i, off, vma, len, flags) in [(0, 0, 0x700000, 0x180, 4), (1, 0x1000, 0x702000, 3, 5)] {
        let at = 64 + i * 56;
        u32le(&mut b, at, 1);
        u32le(&mut b, at + 4, flags);
        u64le(&mut b, at + 8, off);
        u64le(&mut b, at + 16, vma);
        u64le(&mut b, at + 24, vma);
        u64le(&mut b, at + 32, len);
        u64le(&mut b, at + 40, len);
        u64le(&mut b, at + 48, 0x1000);
    }
    b[0x1000..0x1003].copy_from_slice(&[0x89, 0xf8, 0xc3]);
    // Section 1 .text, section 2 symtab, section 3 strtab (names unnecessary).
    u32le(&mut b, 0x440 + 4, 1);
    u64le(&mut b, 0x440 + 8, 6);
    u64le(&mut b, 0x440 + 16, 0x702000);
    u64le(&mut b, 0x440 + 24, 0x1000);
    u64le(&mut b, 0x440 + 32, 3);
    u32le(&mut b, 0x480 + 4, 2);
    u64le(&mut b, 0x480 + 24, 0x300);
    u64le(&mut b, 0x480 + 32, if stripped { 24 } else { 48 });
    u32le(&mut b, 0x480 + 40, 3);
    u64le(&mut b, 0x480 + 56, 24);
    u32le(&mut b, 0x4c0 + 4, 3);
    u64le(&mut b, 0x4c0 + 24, 0x350);
    u64le(&mut b, 0x4c0 + 32, 9);
    b[0x350..0x359].copy_from_slice(b"\0observe\0");
    u32le(&mut b, 0x318, 1);
    b[0x31c] = 0x12;
    u16le(&mut b, 0x31e, 1);
    u64le(&mut b, 0x320, 0x702000);
    u64le(&mut b, 0x328, 3);
    b
}
