# JVM frontend reference

JADX was cloned into the ignored `target/reference/jadx/` directory for this
implementation. Reference commit: `7b109049c28a5f5ad6f3c600ac66a96645c07d24`.

Relevant upstream files, under `jadx-plugins/jadx-java-input/src/main/java/jadx/plugins/input/java/`:

- `data/code/JavaInsnsRegister.java`: JVM instruction inventory and stack operations.
- `data/code/CodeDecodeState.java`: mapping local slots and operand-stack positions to virtual registers.
- `data/code/decoders/InvokeDecoder.java`: method descriptors and invocation arguments.
- `data/ConstPoolReader.java`: constant-pool references and modified UTF-8.

`src/opcodes.rs` adapts the upstream opcode-name table. `src/code.rs` implements
the stack-to-register approach in Rust, using a bounded worklist rather than
JADX's sequential jump-stack state. Stack permutations use scratch-register
copies before overwriting destinations. Branch and exception targets are checked
against instruction boundaries before CFG construction.

The class reader is a new Rust implementation; it does not restore the
platform-symbol builder the DEX front end carries. Method bodies are lifted into
the vendored radguard backend IR, which supplies the analysis, structuring and
Java rendering through `radguard::backend`; nothing in the DEX reader changes.

JADX is Apache-2.0, copyright Skylot and contributors. Its LICENSE and NOTICE are
preserved in this directory. No Java source, JAR tool, JVM, D8 or dx is built or
invoked by rxd. The reference checkout is development material, not a dependency.

Format and instruction semantics were cross-checked against the
[Java SE 21 JVM specification](https://docs.oracle.com/javase/specs/jvms/se21/html/index.html).
