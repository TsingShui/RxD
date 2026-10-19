;; Fixture that forces instruction-view degradation: v128.load is outside the
;; covered opcode set, so `get` returns reason UNSUPPORTED_OPCODE: v128.load.
(module
  (memory 1)
  (func (result v128)
    (i32.const 0)
    (v128.load)))
