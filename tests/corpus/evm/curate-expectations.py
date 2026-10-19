"""Independent acceptance facts inspected against pinned solc ASM and source.

Run after generate.mjs. This never calls rxd or executes an input program.
Offsets are deliberately pinned facts, not values rediscovered from rxd output.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SPECS = {
    "authored-navigation": [
        ("addSeven(uint256)", "Returns calldata word at byte 4 plus 7 modulo 2^256.",
         [537, 539, 541, 542, 544, 545]),
        ("choose(uint256)", "If calldata word 4 is less than 10, returns x+7; otherwise returns x-3, modulo 2^256.",
         [622, 624, 625, 628, 629, 630, 633, 634, 636, 638, 639, 640, 641, 643, 644, 646, 647]),
        ("sum(uint256)", "Loop i=0 while i<n: result+=i and increment i, all modulo 2^256; returns result.",
         [568, 569, 572, 573, 576, 586, 587, 588, 591, 595, 597, 600]),
        ("storeAndRead(uint256)", "Stores calldata word 4 into persistent slot 0; forwards that value and returns x+1 modulo 2^256 without a second SLOAD.",
         [418, 420, 422, 423, 424, 430, 432, 433]),
        ("readValue()", "Loads persistent storage slot 0 and returns its word.",
         [453, 454, 460, 461]),
        ("transientValue(uint256)", "Stores calldata word 4 into transient slot 0x42, then loads/returns it.",
         [134, 136, 137, 139, 142, 144, 150, 151]),
    ],
    "openzeppelin-erc20": [
        ("decimals()", "Returns 18 from the unmodified ERC20 source; no state access in this target.",
         [620, 623, 624]),
        ("totalSupply()", "Loads persistent slot 2, independently identified by Solidity storageLayout as _totalSupply, and returns it.",
         [868, 870, 876, 877]),
    ],
}

out = {
    "provenance": "Independent solc 0.8.26 methodIdentifiers + source/ASM inspection + byte decoder. No rxd output, IDs, or pseudocode snapshots.",
    "samples": [],
}
for slug, cases in SPECS.items():
    oracle = json.loads((ROOT / (slug + ".oracle.json")).read_text())
    raw = (ROOT / (slug + ".runtime.bin")).read_bytes()
    instructions = {ins["offset"]: ins for ins in oracle["instructions"]}
    regions = {reg["signature"]: reg for reg in oracle["restricted_static_regions"]}
    sample = {"sample": slug, "runtime_sha256": hashlib.sha256(raw).hexdigest(),
              "functions": [], "metadata": oracle["metadata"]}
    for signature, behavior, offsets in cases:
        region = regions[signature]
        blocks = [{"start": block["start"], "end": block["end"],
                   "raw_hex": raw[block["start"]:block["end"]].hex()}
                  for block in region["blocks"]]
        facts = [{key: instructions[offset][key]
                  for key in ("offset", "end", "opcode", "nm", "raw", "immediate")
                  if key in instructions[offset]} for offset in offsets]
        sample["functions"].append({"signature": signature, "selector": region["selector"],
                                    "entry_offset": region["entry_offset"], "source_behavior": behavior,
                                    "expected_static_blocks": blocks,
                                    "has_unresolved_jump": region["has_unresolved_jump"],
                                    "instruction_facts": facts})
    if slug == "authored-navigation":
        sample["immediate_literal"] = {
            "selector": oracle["method_identifiers"]["message()"], "opcode_offset": 213,
            "start": 214, "end": 246, "raw_hex": raw[214:246].hex(),
            "utf8": raw[214:246].decode(), "search_prefix": "RXD_DATA_NAVIGATION",
        }
        sample["shared_block"] = {
            "start": 152, "end": 156, "raw_hex": raw[152:156].hex(),
            "source_fact": "All eight selector-dispatched wrappers branch to the same empty revert block.",
        }
    out["samples"].append(sample)

oracle = json.loads((ROOT / "authored-navigation-legacy.oracle.json").read_text())
raw = (ROOT / "authored-navigation-legacy.runtime.bin").read_bytes()
out["samples"].append({
    "sample": "authored-navigation-legacy", "runtime_sha256": hashlib.sha256(raw).hexdigest(),
    "data_literal": oracle["compiler_data"][0],
    "copy_evidence": {"opcode_offset": 397, "offset_push": 391, "size_push": 394,
                      "source_start": 492, "source_end": 592, "copy_size": 100,
                      "block_start": 370, "unresolved_return_jump": 401,
                      "selected_function_selector": oracle["method_identifiers"]["message()"],
                      "selected_function_entry": 276},
    "metadata": oracle["metadata"],
})
(ROOT / "acceptance-expectations.json").write_text(json.dumps(out, indent=2) + "\n")
