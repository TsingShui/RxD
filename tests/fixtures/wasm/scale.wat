;; Fixture for memory index scaling in decompilation output:
;; static byte offset scales (a[12]:int -> a[3]:int) and a dynamic
;; (base + (i << 2)) address folds to base[i]:int.
(module
  (memory 1)
  (func (param $base i32) (param $i i32) (result i32)
    ;; (base + (i<<2))[0]  -> base[i]:int
    (local.set $base
      (i32.load (i32.add (local.get $base) (i32.shl (local.get $i) (i32.const 2)))))
    ;; base[12]:int -> base[3]:int
    (i32.load offset=12 (local.get $base))))
