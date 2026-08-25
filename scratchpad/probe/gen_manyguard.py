#!/usr/bin/env python3
# Clean gap demo for frontier (a): ctrl is a 2-state FSM whose return is gated on `data == 42`
# (data 48-bit, data'=data => invariant, recoverable). A SEPARATE decoy register `mode` is compared
# against constants 2..9 in DEAD logic (mode is NOT in ctrl's cone-of-influence). The un-directed
# eager extraction seeds ALL eq atoms (data==42 + mode==2..9 = 9 guards), crowding MAX_AUTO_SEED and
# blowing up the cube; COI-directed extraction seeds ONLY data==42 (mode outside ctrl's COI) and
# decides fast. AG EF (ctrl==0) HOLDS.
import sys
W = int(sys.argv[1]) if len(sys.argv) > 1 else 48
K = 42
L = []
L.append("1 sort bitvec 4")     # ctrl / mode width
L.append("2 sort bitvec 1")     # bool
L.append(f"3 sort bitvec {W}")  # data
L.append("4 state 1 ctrl")
L.append("5 state 3 data")
L.append("6 state 1 mode")      # decoy register (NOT in ctrl's COI)
L.append("10 zero 1")           # idle=0
L.append("11 one 1")            # busy=1
L.append(f"12 constd 3 {K}")
# decoy constants 2..9
nid = 20
dc = {}
for k in range(2, 10):
    L.append(f"{nid} constd 1 {k}"); dc[k] = nid; nid += 1
# init: ctrl=1 (busy), data=K, mode=2
L.append("13 init 1 4 11")
L.append("14 init 3 5 12")
L.append(f"15 init 1 6 {dc[2]}")
# REAL return guards: busy && data==K
L.append(f"{nid} eq 2 4 11"); busy = nid; nid += 1
L.append(f"{nid} eq 2 5 12"); dK = nid; nid += 1
L.append(f"{nid} and 2 {busy} {dK}"); ret = nid; nid += 1
L.append(f"{nid} ite 1 {ret} 10 4"); ctrl_next = nid; nid += 1   # busy&&data==K -> idle(0) else stay
L.append(f"{nid} next 1 4 {ctrl_next}"); nid += 1
L.append(f"{nid} next 3 5 5"); nid += 1                          # data'=data
# decoy `mode` ring using eq(mode,k) for k=2..9 (dead: does not touch ctrl)
chain = 6
for k in range(2, 10):
    nxt = dc[k+1] if (k+1) in dc else dc[2]
    L.append(f"{nid} eq 2 6 {dc[k]}"); eqk = nid; nid += 1
    L.append(f"{nid} ite 1 {eqk} {nxt} {chain}"); chain = nid; nid += 1
L.append(f"{nid} next 1 6 {chain}"); nid += 1                    # mode' = ring(mode)
print("\n".join(L))
