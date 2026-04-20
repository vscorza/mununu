// CWE-1260: Memory Region Address Overlap — Fix
//
// FIX: UART_LEN reduced from 0x12 to 0x10 — regions no longer overlap.
// UART: 0x10..0x20, AES: 0x20..0x28.
//
// Reference: cwe.mitre.org/data/definitions/1260.html

module cwe1260_addr_overlap_fixed(
    input  logic       clk,
    input  logic       rst,
    input  logic [7:0] addr,
    input  logic       req
);

    localparam UART_BASE = 8'h10;
    localparam UART_LEN  = 8'h10;  // FIX: 0x10 — no overlap
    localparam AES_BASE  = 8'h20;
    localparam AES_LEN   = 8'h08;

    typedef enum logic [1:0] {IDLE, UART_ACCESS, AES_ACCESS} state_t;
    state_t state;

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
