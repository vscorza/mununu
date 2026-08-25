(set-logic QF_UFBV)
(set-option :produce-interpolants true)
(declare-fun a () (_ BitVec 8))
(declare-fun b () (_ BitVec 8))
; A side: a == b (source states share this relation)
(assert (= a b))
; ask for an interpolant separating A from B = (bvult a b)  [a < b]
(get-interpolant I (bvult a b))
