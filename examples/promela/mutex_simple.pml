// Peterson-style mutual exclusion (Promela MVP example for the mununu Promela adapter).
//
// Two processes (P0, P1) compete for a critical section guarded by Peterson's
// algorithm. The LTL property `mutex` claims both processes never hold the
// critical-section flag (cs0/cs1) simultaneously.

byte turn = 0;
bool flag0 = false;
bool flag1 = false;
bool cs0 = false;
bool cs1 = false;

active proctype P0() {
    do
    :: true ->
        flag0 = true;
        turn = 1;
        (flag1 == false || turn == 0);
        cs0 = true;
        cs0 = false;
        flag0 = false;
    od
}

active proctype P1() {
    do
    :: true ->
        flag1 = true;
        turn = 0;
        (flag0 == false || turn == 1);
        cs1 = true;
        cs1 = false;
        flag1 = false;
    od
}

ltl mutex { [] !(cs0 && cs1) }
