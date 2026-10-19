;; Deterministic hand-written fixture for rxd wasm structural tests.
;; Covers: imported func (merged index space), defined funcs with a call and
;; ref.func, exports, a table/memory/global, and a data segment holding both
;; valid UTF-8 and an invalid byte run.
(module
  (import "env" "log" (func $log (param i32)))      ;; func index 0 (imported)
  (table 1 funcref)
  (memory (export "memory") 1)
  (global $g (mut i32) (i32.const 7))

  (func $sum (param $n i32) (result i32)            ;; func index 1 (defined)
    (local $i i32) (local $acc i32)
    (local.set $acc (i32.const 0))
    (local.set $i (i32.const 1))
    (block $done
      (loop $loop
        (br_if $done (i32.gt_s (local.get $i) (local.get $n)))
        (local.set $acc (i32.add (local.get $acc) (local.get $i)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $acc))

  (func $run (result i32)                           ;; func index 2 (defined)
    (call $log (i32.const 42))
    (drop (ref.func $sum))
    (call $sum (i32.const 10)))

  (export "run" (func $run))
  (export "sum" (func $sum))
  (elem declare func $sum)
  (data (i32.const 0) "hello\00\ff\feworld"))
