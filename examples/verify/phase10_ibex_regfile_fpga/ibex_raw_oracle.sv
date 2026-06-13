// Oracle wrapper: read-after-write on ibex_register_file_fpga.
module ibex_raw_oracle (
  input logic        clk_i,
  input logic        rst_ni,
  input logic [4:0]  waddr_a_i,
  input logic [3:0]  wdata_a_i,
  input logic        we_a_i
);
  logic [3:0] rdata_a_o, rdata_b_o;

  logic        saw_write = 1'b0;
  logic [4:0]  saved_addr = 5'd0;
  logic [3:0]  saved_data = 4'd0;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) begin
      saw_write  <= 1'b0;
      saved_addr <= '0;
      saved_data <= '0;
    end else begin
      saw_write  <= we_a_i && (waddr_a_i != 5'd0);
      saved_addr <= waddr_a_i;
      saved_data <= wdata_a_i;
    end
  end

  ibex_register_file_fpga #(.RV32E(1), .DataWidth(4)) rf (
    .clk_i, .rst_ni,
    .test_en_i(1'b0), .dummy_instr_id_i(1'b0), .dummy_instr_wb_i(1'b0),
    .raddr_a_i(saved_addr), .raddr_b_i(5'd0),
    .rdata_a_o(rdata_a_o), .rdata_b_o(rdata_b_o),
    .waddr_a_i(waddr_a_i), .wdata_a_i(wdata_a_i), .we_a_i(we_a_i)
  );

  // One cycle after a write to a nonzero address, reading that address
  // returns the written data — unless this cycle overwrites it.
  always @(*) begin
    if (rst_ni && saw_write && !(we_a_i && (waddr_a_i == saved_addr)))
      assert (rdata_a_o == saved_data);
  end
endmodule
