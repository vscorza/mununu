// CWE-1262: CSR Privilege Level Bypass — Fix
//
// FIX: No MEPC exception. Uniform privilege check.
//
// Reference: cwe.mitre.org/data/definitions/1262.html

module cwe1262_csr_bypass_fixed(
    input  logic        clk,
    input  logic        rst,
    input  logic [11:0] csr_addr,
    input  logic [1:0]  priv_lvl,
    input  logic        csr_req
);

    typedef enum logic [1:0] {IDLE, GRANTED, DENIED} state_t;
    state_t state;

    logic priv_ok;
    logic bypass;

    assign priv_ok = (priv_lvl >= csr_addr[9:8]);  // FIX: no exception
    assign bypass  = 0;                              // FIX: bypass never fires

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else begin
            case (state)
                IDLE: begin
                    if (csr_req) begin
                        if (priv_ok)
                            state <= GRANTED;
                        else
                            state <= DENIED;
                    end
                end
                GRANTED: state <= IDLE;
                DENIED:  state <= IDLE;
            endcase
        end
    end

endmodule
