(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun data0 () (_ BitVec 48))
(declare-fun target0 () (_ BitVec 48))
(declare-fun data1 () (_ BitVec 48))
(declare-fun target1 () (_ BitVec 48))
; A: a transition step FROM a state where the (unstated-in-design) relation holds
(assert (and (= data1 (bvadd data0 (_ bv1 48)))
             (= target1 (bvadd target0 (_ bv1 48)))
             (= data0 target0)))
; B (goal): next state satisfies the relation. A => B holds; interpolant over shared {data1,target1}.
(get-interpolant I (= data1 target1))
