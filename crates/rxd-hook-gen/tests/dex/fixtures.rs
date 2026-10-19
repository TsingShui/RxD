//! Authored table-level DEX 039/041 fixtures; Apache-2.0. Never executed.
//! Reuses the repository's handcrafted layout approach, with independent method facts.
use std::collections::BTreeSet;
pub const CLASS: &str = "Lexample/原$Inner;";
#[derive(Clone)]
pub struct Method<'a> {
    pub owner: &'a str,
    pub name: &'a str,
    pub parameters: Vec<&'a str>,
    pub result: &'a str,
    pub flags: u32,
}
pub fn method<'a>(
    name: &'a str,
    parameters: &[&'a str],
    result: &'a str,
    flags: u32,
) -> Method<'a> {
    Method {
        owner: CLASS,
        name,
        parameters: parameters.to_vec(),
        result,
        flags,
    }
}
pub fn methods() -> Vec<Method<'static>> {
    let mut external = method("external", &[], "V", 0x100);
    external.owner = "La/Other;";
    vec![
        external,
        method("<clinit>", &[], "V", 8),
        method("<init>", &["I"], "V", 0x10001),
        method("abstracted", &[], "V", 0x401),
        method("nativeCall", &["J"], "J", 0x101),
        method("overload", &["I"], "I", 1),
        method("overload", &["Ljava/lang/String;"], "Ljava/lang/String;", 1),
        method(
            "staticVoid",
            &["[I", "[[Ljava/lang/String;", "C", "Ljava/lang/Object;"],
            "V",
            9,
        ),
        method(
            "unicode$λ",
            &["Z", "B", "S", "F", "D", "[J", "[Lexample/原$Inner;"],
            "[I",
            0x1041,
        ),
    ]
}
pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn uleb(out: &mut Vec<u8>, mut n: u32) {
    loop {
        let byte = (n & 127) as u8;
        n >>= 7;
        out.push(byte | if n != 0 { 128 } else { 0 });
        if n == 0 {
            break;
        }
    }
}
fn shorty(method: &Method<'_>) -> String {
    std::iter::once(method.result)
        .chain(method.parameters.iter().copied())
        .map(|s| {
            if s.starts_with(['L', '[']) {
                'L'
            } else {
                s.chars().next().unwrap()
            }
        })
        .collect()
}
pub fn dex(methods: &[Method<'_>]) -> Vec<u8> {
    build(methods, 0, false)
}
pub fn container(methods: &[Method<'_>]) -> Vec<u8> {
    let mut first = build(methods, 0, true);
    let second = build(methods, first.len() as u32, true);
    let offset = first.len();
    first.extend_from_slice(&second);
    let size = first.len() as u32;
    put32(&mut first, 0x70, size);
    put32(&mut first, offset + 0x70, size);
    first
}
fn build(methods: &[Method<'_>], base: u32, v041: bool) -> Vec<u8> {
    let mut types = BTreeSet::from([CLASS.to_owned()]);
    let mut strings = BTreeSet::new();
    for m in methods {
        types.insert(m.owner.to_owned());
        types.insert(m.result.to_owned());
        types.extend(m.parameters.iter().map(|s| s.to_string()));
        strings.insert(m.name.to_owned());
        strings.insert(shorty(m));
    }
    strings.extend(types.iter().cloned());
    let strings: Vec<_> = strings.into_iter().collect();
    let types: Vec<_> = types.into_iter().collect();
    let sid = |s: &str| strings.iter().position(|v| v == s).unwrap() as u32;
    let tid = |s: &str| types.iter().position(|v| v == s).unwrap() as u32;
    let mut protos = methods
        .iter()
        .map(|m| {
            (
                tid(m.result),
                m.parameters.iter().map(|s| tid(s)).collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    protos.sort();
    protos.dedup();
    let pid = |m: &Method<'_>| {
        protos
            .iter()
            .position(|p| {
                p.0 == tid(m.result)
                    && p.1 == m.parameters.iter().map(|s| tid(s)).collect::<Vec<_>>()
            })
            .unwrap() as u16
    };
    let mut methods = methods.to_vec();
    methods.sort_by_key(|m| (tid(m.owner), sid(m.name), pid(m)));
    let strings_off = if v041 { 0x78 } else { 0x70 };
    let types_off = strings_off + strings.len() * 4;
    let protos_off = types_off + types.len() * 4;
    let methods_off = protos_off + protos.len() * 12;
    let classes_off = methods_off + methods.len() * 8;
    let data_off = classes_off + 32;
    let mut out = vec![0; data_off];
    out[..8].copy_from_slice(if v041 { b"dex\n041\0" } else { b"dex\n039\0" });
    put32(&mut out, 0x24, strings_off as u32);
    put32(&mut out, 0x28, 0x12345678);
    for (size_at, off_at, count, off) in [
        (0x38, 0x3c, strings.len(), strings_off),
        (0x40, 0x44, types.len(), types_off),
        (0x48, 0x4c, protos.len(), protos_off),
        (0x58, 0x5c, methods.len(), methods_off),
        (0x60, 0x64, 1, classes_off),
    ] {
        put32(&mut out, size_at, count as u32);
        put32(&mut out, off_at, off as u32 + base);
    }
    for (n, s) in strings.iter().enumerate() {
        let offset = out.len() as u32 + base;
        put32(&mut out, strings_off + n * 4, offset);
        uleb(&mut out, s.encode_utf16().count() as u32);
        // MUTF-8 is UTF-8 for these authored BMP names; encode surrogate pairs for astral names.
        for unit in s.encode_utf16() {
            if unit == 0 {
                out.extend_from_slice(&[0xc0, 0x80]);
            } else if unit < 0x80 {
                out.push(unit as u8);
            } else if unit < 0x800 {
                out.extend_from_slice(&[(0xc0 | unit >> 6) as u8, (0x80 | (unit & 63)) as u8]);
            } else {
                out.extend_from_slice(&[
                    (0xe0 | unit >> 12) as u8,
                    (0x80 | ((unit >> 6) & 63)) as u8,
                    (0x80 | (unit & 63)) as u8,
                ]);
            }
        }
        out.push(0);
    }
    for (n, t) in types.iter().enumerate() {
        put32(&mut out, types_off + n * 4, sid(t));
    }
    for (n, (ret, params)) in protos.iter().enumerate() {
        let m = methods.iter().find(|m| pid(m) as usize == n).unwrap();
        put32(&mut out, protos_off + n * 12, sid(&shorty(m)));
        put32(&mut out, protos_off + n * 12 + 4, *ret);
        if !params.is_empty() {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let offset = out.len() as u32 + base;
            put32(&mut out, protos_off + n * 12 + 8, offset);
            out.extend_from_slice(&(params.len() as u32).to_le_bytes());
            for param in params {
                out.extend_from_slice(&(*param as u16).to_le_bytes());
            }
        }
    }
    let mut direct = Vec::new();
    let mut virtuals = Vec::new();
    for (n, m) in methods.iter().enumerate() {
        put16(&mut out, methods_off + n * 8, tid(m.owner) as u16);
        put16(&mut out, methods_off + n * 8 + 2, pid(m));
        put32(&mut out, methods_off + n * 8 + 4, sid(m.name));
        if m.owner != CLASS {
            continue;
        }
        let code = if m.flags & (0x100 | 0x400) != 0 {
            0
        } else {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let code = out.len() as u32 + base;
            let ins = u16::from(m.flags & 8 == 0)
                + m.parameters
                    .iter()
                    .map(|s| if *s == "J" || *s == "D" { 2 } else { 1 })
                    .sum::<u16>();
            out.extend_from_slice(&(ins + 2).to_le_bytes());
            out.extend_from_slice(&ins.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            let instructions: &[u16] = match m.result {
                "V" => &[0x000e],
                "J" | "D" => &[0x0016, 0, 0x0010],
                d if d.starts_with(['L', '[']) => &[0x0012, 0x0011],
                _ => &[0x0012, 0x000f],
            };
            out.extend_from_slice(&(instructions.len() as u32).to_le_bytes());
            for i in instructions {
                out.extend_from_slice(&i.to_le_bytes());
            }
            code
        };
        let rows = if m.flags & (8 | 2 | 0x10000) != 0 {
            &mut direct
        } else {
            &mut virtuals
        };
        rows.push((n as u32, m.flags, code));
    }
    let class_data = out.len() as u32 + base;
    uleb(&mut out, 0);
    uleb(&mut out, 0);
    uleb(&mut out, direct.len() as u32);
    uleb(&mut out, virtuals.len() as u32);
    for rows in [direct, virtuals] {
        let mut last = 0;
        for (idx, flags, code) in rows {
            uleb(&mut out, idx - last);
            uleb(&mut out, flags);
            uleb(&mut out, code);
            last = idx;
        }
    }
    put32(&mut out, classes_off, tid(CLASS));
    put32(&mut out, classes_off + 4, 1);
    put32(&mut out, classes_off + 8, u32::MAX);
    put32(&mut out, classes_off + 24, class_data);
    let size = out.len() as u32;
    put32(&mut out, 0x20, size);
    put32(&mut out, 0x68, size - data_off as u32);
    put32(&mut out, 0x6c, data_off as u32 + base);
    if v041 {
        put32(&mut out, 0x70, size);
        put32(&mut out, 0x74, base);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &out[12..] {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    put32(&mut out, 8, b << 16 | a);
    out
}
