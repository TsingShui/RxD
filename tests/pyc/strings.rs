use super::*;

fn strings(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "strings"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn bytes_field(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b's'];
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}
fn module(body: &[u8], consts: &[u8]) -> Vec<u8> {
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(body);
    out.extend_from_slice(consts);
    out.extend_from_slice(b")\0)\0");
    out.extend(bytes_field(b""));
    out.extend_from_slice(b"z\x0afixture.pyz\x08<module>z\x08<module>");
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend(bytes_field(b""));
    out.extend(bytes_field(b""));
    out
}
#[test]
fn pyc_strings_budget_work_includes_actual_hex_generation_before_allocation() {
    let payload = vec![b'a'; 500];
    let mut consts = b")\x01".to_vec();
    consts.extend(bytes_field(&payload));
    let (_dir, path) = fixture(module(&bytes_field(&[]), &consts), "work.marshal");
    let zero = strings(
        &path,
        &["--version", "3.12", "--filter", "missing", "--perf"],
    );
    let reports: Vec<Value> = String::from_utf8(zero.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let without_output = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["work"]
        .as_u64()
        .unwrap();
    let all = strings(&path, &["--version", "3.12", "--perf"]);
    success(&all);
    let reports: Vec<Value> = String::from_utf8(all.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let with_output = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["work"]
        .as_u64()
        .unwrap();
    // Missing-byte matching visits 494 bytes; raw hex still processes all 500.
    assert!(with_output > without_output);
}
