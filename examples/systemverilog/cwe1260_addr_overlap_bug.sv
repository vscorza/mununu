// CWE-1260: Memory Region Address Overlap — Bug
//
// Based on MITRE CWE-1260 (Improper Handling of Overlap Between Protected
// Memory Ranges). Real-world pattern in SoC address decoders.
//
// BUG: UART region (base=0x10, length=0x12) extends to 0x22, overlapping
// with AES region (base=0x20, length=0x08). Addresses 0x20-0x21 match both.
//
// The overlap is a combinational output — no detector register needed.
// The property checks the wire directly via "combinational": true.

module cwe1260_addr_overlap_bug(
    input  logic       clk,
    input  logic       rst,
    input  logic [7:0] addr,
    input  logic       req
);

    localparam UART_BASE = 8'h10;
    localparam UART_LEN  = 8'h12;  // BUG: extends to 0x22, past AES_BASE
    localparam AES_BASE  = 8'h20;
    localparam AES_LEN   = 8'h08;

    typedef enum logic [1:0] {IDLE, UART_ACCESS, AES_ACCESS} state_t;
    state_t state;

    // Combinational address decode outputs
    logic uart_sel;
    logic aes_sel;
    logic overlap;

    assign uart_sel = (addr >= UART_BASE) && (addr < UART_BASE + UART_LEN);
    assign aes_sel  = (addr >= AES_BASE)  && (addr < AES_BASE + AES_LEN);
    assign overlap  = uart_sel && aes_sel;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else begin
            case (state)
                IDLE: begin
                    if (req && uart_sel)
                        state <= UART_ACCESS;
                    else if (req && aes_sel)
                        state <= AES_ACCESS;
                end
                UART_ACCESS: state <= IDLE;
                AES_ACCESS: state <= IDLE;
            endcase
        end
    end

endmodule
