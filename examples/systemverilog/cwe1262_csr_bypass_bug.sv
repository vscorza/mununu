// CWE-1262: CSR Privilege Level Bypass — Bug
//
// Based on MITRE CWE-1262 and CVA6 Hack@DAC 2019 bug.
//
// BUG: Exception for CSR address 0x341 (MEPC) allows user-mode access
// to a machine-level CSR. The bypass condition is a combinational wire.

module cwe1262_csr_bypass_bug(
    input  logic        clk,
    input  logic        rst,
    input  logic [11:0] csr_addr,
    input  logic [1:0]  priv_lvl,
    input  logic        csr_req
);

    typedef enum logic [1:0] {IDLE, GRANTED, DENIED} state_t;
    state_t state;

    // Combinational privilege check with MEPC exception
    logic priv_ok;
    logic bypass;

    assign priv_ok = (priv_lvl >= csr_addr[9:8]) || (csr_addr == 833);  // BUG: MEPC exception
    assign bypass  = (priv_lvl < csr_addr[9:8]) && (csr_addr == 833);   // bypass indicator

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
