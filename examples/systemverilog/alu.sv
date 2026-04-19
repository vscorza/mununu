// Simple ALU with Accumulator
// Demonstrates: register-based Kripke construction with value-mapped enum
//
// The ALU has a 4-bit accumulator driven by external command and operand
// inputs. The command input is abstracted to a value-mapped enum — only
// the specific opcode values used in the case statement matter.
//
// Properties verified:
//   safety: the automaton is well-formed (no deadlock)
//
// @mununu mode kripke
// @mununu domain acc: bounded_counter 0..7
// @mununu domain cmd: enum {NOP=0, LOAD=1, ADD=2, SUB=3, CLR=4, OTHER}
// @mununu domain operand: bounded_counter 0..3
// @mununu input cmd, operand, start
// @mununu ltl safety: nu X. ([] X)
module alu(
    input  logic       clk,
    input  logic       rst,
    input  logic       start,
    input  logic [7:0] cmd,
    input  logic [3:0] operand,
    output logic [3:0] result
);

    logic [3:0] acc;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            acc <= 0;
        end else if (start) begin
            case (cmd)
                0: ;                       // NOP
                1: acc <= operand;          // LOAD
                2: acc <= acc + operand;    // ADD
                3: acc <= acc - operand;    // SUB
                4: acc <= 0;               // CLR
                default: ;                 // OTHER — unknown op, no effect
            endcase
        end
    end

    assign result = acc;

endmodule
