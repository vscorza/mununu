// charge_commit.sv — a command/commit controller with an arithmetic charge datapath.
//
// The controller walks four phases IDLE -> CHARGE -> READY -> COMMIT, accumulating
// a charge level `lvl` while charging (`lvl += STEP` per `pulse`) and only advancing
// to READY once `lvl >= THRESHOLD`. The safety obligation is
//
//     G ( commit_en -> lvl >= THRESHOLD )
//
// i.e. it may only assert `commit_en` (open the commit window) while charged.
// This is the worked example of docs/design/predicate-abstraction-worked-example.md.
//
// Two surfaces consume this file directly against real binaries:
//   * sv2v + yosys `write_btor`  -> examples/hw/charge_commit.btor2   (mununu btor2 cegar)
//   * verilator --binary --assert -> examples/hw/charge_commit_tb.sv  (bounded simulation)
module charge_commit (
    input  logic clk,
    input  logic rst,
    input  logic req,     // environment: a request arrives
    input  logic pulse,   // environment: a charge pulse
    input  logic fire,    // controller: open the commit window
    input  logic clr,     // environment: release
    output logic commit_en
);
    typedef enum logic [1:0] { IDLE = 2'd0, CHARGE = 2'd1, READY = 2'd2, COMMIT = 2'd3 } phase_t;
    phase_t     phase;
    logic [7:0] lvl;            // charge level — the arithmetic datapath

    localparam logic [7:0] STEP      = 8'd2;
    localparam logic [7:0] THRESHOLD = 8'd6;

    always_ff @(posedge clk) begin
        if (rst) begin
            phase <= IDLE;
            lvl   <= 8'd0;
        end else begin
            case (phase)
                IDLE  : if (req) phase <= CHARGE;
                CHARGE: begin
                    if (lvl < THRESHOLD) begin
                        if (pulse) lvl <= lvl + STEP;   // arithmetic: accumulate
                    end else begin
                        phase <= READY;                  // arithmetic guard: lvl >= 6
                    end
                end
                READY : if (fire) phase <= COMMIT;
                COMMIT: if (clr) begin phase <= IDLE; lvl <= 8'd0; end
                default: ;
            endcase
        end
    end

    assign commit_en = (phase == COMMIT);

    // Safety obligation, as a clocked immediate assertion guarded by FORMAL.
    // Verilator checks it every posedge under `--assert -DFORMAL`; yosys lowers
    // it to the BTOR2 `bad` state under `read_verilog -sv -formal -DFORMAL`.
`ifdef FORMAL
    always_ff @(posedge clk) begin
        if (!rst) begin
            safety_charged: assert (!commit_en || (lvl >= THRESHOLD));
        end
    end
`endif
endmodule
