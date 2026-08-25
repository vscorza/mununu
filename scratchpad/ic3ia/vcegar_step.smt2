(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun a0 () (_ BitVec 16))
(declare-fun b0 () (_ BitVec 16))
(declare-fun a1 () (_ BitVec 16))
(declare-fun b1 () (_ BitVec 16))
; A: a reachable state (a<200, the safety region) AND one transition step
(assert (and (bvult a0 (_ bv200 16))
             (= a1 (ite (bvult a0 (_ bv100 16)) (bvadd a0 b0) a0))
             (= b1 a0)))
; B (goal, A=>B): the next state is still in the safety region a1<200.  Interpolant over shared {a1,b1}.
(get-interpolant I (bvult a1 (_ bv200 16)))
