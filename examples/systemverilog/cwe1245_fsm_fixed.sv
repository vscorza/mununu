// CWE-1245: FSM with Undefined States — Security Fix
//
// FIX: default case forces FSM back to IDLE on any undefined state,
// and an error flag is asserted to signal the fault.
//
// Reference: cwe.mitre.org/data/definitions/1245.html

module cwe1245_fsm_fixed(
    input  logic       clk,
    input  logic       rst,
    input  logic       req_read,
    input  logic       req_write,
    input  logic       grant,
    input  logic       glitch,
    output logic       access_ok
);

    typedef enum logic [2:0] {
        IDLE    = 3'b001,
        READING = 3'b010,
        WRITING = 3'b100
    } state_t;

    logic [2:0] state;
    logic       err_flag;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state    <= IDLE;
            err_flag <= 0;
        end else if (glitch) begin
            state <= 3'b101;
        end else begin
            case (state)
                IDLE: begin
                    err_flag <= 0;
                    if (req_read)       state <= READING;
                    else if (req_write) state <= WRITING;
                end
                READING: begin
                    if (grant) state <= IDLE;
                end
                WRITING: begin
                    if (grant) state <= IDLE;
                end
                default: begin
                    // FIX: recover from undefined state
                    state    <= IDLE;
                    err_flag <= 1;
                end
            endcase
        end
    end

    assign access_ok = (state == READING) || (state == WRITING);

endmodule
