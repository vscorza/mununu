// Bounded Buffer (2-entry)
//
// Capacity: 2 entries. Count tracks occupancy (0..3 where 3 = overflow).
// The full flag is combinational from the current count (no delay).
// Overflow: if push occurs when count is already 2, count goes to 3.

module bounded_buffer(
    input  logic clk,
    input  logic rst,
    input  logic push,
    input  logic pop,
    output logic full
);

    logic [1:0] count;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            count <= 0;
        end else begin
            if (push && !pop)
                count <= count + 1;
            else if (pop && !push) begin
                if (count > 0)
                    count <= count - 1;
            end
        end
    end

    // Combinational full: true when count is at capacity (no delay)
    assign full = (count >= 2);

endmodule
