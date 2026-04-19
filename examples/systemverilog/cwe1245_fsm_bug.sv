// CWE-1245: FSM with Undefined States — Security Bug
//
// Based on MITRE CWE-1245 (Improper Finite State Machines in Hardware Logic).
// Real CVEs: CVE-2024-21853, CVE-2024-24968 (Intel processors).
//
// BUG: One-hot encoded FSM with 3 bits for 3 valid states but no default
// case. An attacker can inject a glitch to reach undefined bit patterns
// (e.g., 3'b101), bypassing access control. In the undefined state,
// the access_ok output is undefined (defaults to 0, but the FSM is stuck).
//
// The module controls access to a security-sensitive register.

module cwe1245_fsm_bug(
    input  logic       clk,
    input  logic       rst,
    input  logic       req_read,
    input  logic       req_write,
    input  logic       grant,
    input  logic       glitch,       // models fault injection
    output logic       access_ok
);

    typedef enum logic [2:0] {
        IDLE    = 3'b001,
        READING = 3'b010,
        WRITING = 3'b100
    } state_t;

    logic [2:0] state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else if (glitch) begin
            // Fault injection: attacker pushes FSM to undefined state
            state <= 3'b101;
        end else begin
            case (state)
                IDLE: begin
                    if (req_read)       state <= READING;
                    else if (req_write) state <= WRITING;
                end
                READING: begin
                    if (grant) state <= IDLE;
                end
                WRITING: begin
                    if (grant) state <= IDLE;
                end
                // BUG: no default case — undefined states are absorbing
            endcase
        end
    end

    // Access is granted only in READING or WRITING states
    assign access_ok = (state == READING) || (state == WRITING);

endmodule
