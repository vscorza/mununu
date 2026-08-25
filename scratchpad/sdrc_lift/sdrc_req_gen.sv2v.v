module sdrc_req_gen (
	clk,
	reset_n,
	cfg_colbits,
	sdr_width,
	req,
	req_id,
	req_addr,
	req_len,
	req_wrap,
	req_wr_n,
	req_ack,
	r2x_idle,
	r2b_req,
	r2b_req_id,
	r2b_start,
	r2b_last,
	r2b_wrap,
	r2b_ba,
	r2b_raddr,
	r2b_caddr,
	r2b_len,
	r2b_write,
	b2r_ack,
	b2r_arb_ok,
	page_ovflw
);
	parameter APP_AW = 26;
	parameter APP_DW = 32;
	parameter APP_BW = 4;
	parameter APP_RW = 9;
	parameter SDR_DW = 16;
	parameter SDR_BW = 2;
	input clk;
	input reset_n;
	input [1:0] cfg_colbits;
	input req;
	input [3:0] req_id;
	input [APP_AW - 1:0] req_addr;
	input [APP_RW - 1:0] req_len;
	input req_wr_n;
	input req_wrap;
	output reg req_ack;
	output reg r2x_idle;
	output reg r2b_req;
	output reg r2b_start;
	output wire r2b_last;
	output reg r2b_write;
	output wire r2b_wrap;
	output reg [3:0] r2b_req_id;
	output reg [1:0] r2b_ba;
	output reg [12:0] r2b_raddr;
	output reg [12:0] r2b_caddr;
	output wire [6:0] r2b_len;
	input b2r_ack;
	input b2r_arb_ok;
	input page_ovflw;
	input [1:0] sdr_width;
	reg [1:0] req_st;
	reg [1:0] next_req_st;
	reg req_idle;
	reg req_ld;
	reg lcl_wrap;
	reg [6:0] lcl_req_len;
	reg page_ovflw_r;
	wire [6:0] next_req_len;
	wire [12:0] max_r2b_len;
	reg [12:0] max_r2b_len_r;
	reg [APP_AW - 1:0] curr_sdr_addr;
	wire [APP_AW - 1:0] next_sdr_addr;
	reg [APP_AW:0] req_addr_int;
	reg [APP_RW - 1:0] req_len_int;
	always @(*)
		if (sdr_width == 2'b00) begin
			req_addr_int = {1'b0, req_addr};
			req_len_int = req_len;
		end
		else if (sdr_width == 2'b01) begin
			req_addr_int = {req_addr, 1'b0};
			req_len_int = {req_len, 1'b0};
		end
		else begin
			req_addr_int = {req_addr, 2'b00};
			req_len_int = {req_len, 2'b00};
		end
	assign max_r2b_len = (cfg_colbits == 2'b00 ? 12'h100 - {4'b0000, req_addr_int[7:0]} : (cfg_colbits == 2'b01 ? 12'h200 - {3'b000, req_addr_int[8:0]} : (cfg_colbits == 2'b10 ? 12'h400 - {2'b00, req_addr_int[9:0]} : 12'h800 - {1'b0, req_addr_int[10:0]})));
	assign r2b_len = (r2b_start ? (page_ovflw_r ? max_r2b_len_r : lcl_req_len) : lcl_req_len);
	assign next_req_len = lcl_req_len - r2b_len;
	assign next_sdr_addr = curr_sdr_addr + r2b_len;
	assign r2b_wrap = lcl_wrap;
	assign r2b_last = (r2b_start & !page_ovflw_r) | (req_st == 2'b10);
	always @(posedge clk) begin
		page_ovflw_r <= (req_ack ? page_ovflw : 'h0);
		max_r2b_len_r <= (req_ack ? max_r2b_len : 'h0);
		r2b_start <= (req_ack ? 1'b1 : (b2r_ack ? 1'b0 : r2b_start));
		r2b_write <= (req_ack ? ~req_wr_n : r2b_write);
		r2b_req_id <= (req_ack ? req_id : r2b_req_id);
		lcl_wrap <= (req_ack ? req_wrap : lcl_wrap);
		lcl_req_len <= (req_ack ? req_len_int : (req_ld ? next_req_len : lcl_req_len));
		curr_sdr_addr <= (req_ack ? req_addr_int : (req_ld ? next_sdr_addr : curr_sdr_addr));
	end
	always @(*) begin
		r2x_idle = 1'b0;
		req_idle = 1'b0;
		req_ack = 1'b0;
		req_ld = 1'b0;
		r2b_req = 1'b0;
		next_req_st = 2'b00;
		case (req_st)
			2'b00: begin
				r2x_idle = ~req;
				req_idle = 1'b1;
				req_ack = req & b2r_arb_ok;
				req_ld = 1'b0;
				r2b_req = 1'b0;
				next_req_st = (req & b2r_arb_ok ? 2'b01 : 2'b00);
			end
			2'b01: begin
				r2x_idle = 1'b0;
				req_idle = 1'b0;
				req_ack = 1'b0;
				req_ld = b2r_ack;
				r2b_req = 1'b1;
				next_req_st = (b2r_ack ? (page_ovflw_r ? 2'b10 : 2'b00) : 2'b01);
			end
			2'b10: begin
				r2x_idle = 1'b0;
				req_idle = 1'b0;
				req_ack = 1'b0;
				req_ld = b2r_ack;
				r2b_req = 1'b1;
				next_req_st = (b2r_ack ? 2'b00 : 2'b10);
			end
		endcase
	end
	always @(posedge clk)
		if (~reset_n)
			req_st <= 2'b00;
		else
			req_st <= next_req_st;
	wire [APP_AW - 1:0] map_address;
	assign map_address = (req_ack ? req_addr_int : (req_ld ? next_sdr_addr : curr_sdr_addr));
	always @(posedge clk) begin
		r2b_ba <= (cfg_colbits == 2'b00 ? {map_address[9:8]} : (cfg_colbits == 2'b01 ? {map_address[10:9]} : (cfg_colbits == 2'b10 ? {map_address[11:10]} : map_address[12:11])));
		r2b_caddr <= (cfg_colbits == 2'b00 ? {5'b00000, map_address[7:0]} : (cfg_colbits == 2'b01 ? {4'b0000, map_address[8:0]} : (cfg_colbits == 2'b10 ? {3'b000, map_address[9:0]} : {2'b00, map_address[10:0]})));
		r2b_raddr <= (cfg_colbits == 2'b00 ? map_address[22:10] : (cfg_colbits == 2'b01 ? map_address[23:11] : (cfg_colbits == 2'b10 ? map_address[24:12] : map_address[25:13])));
	end
endmodule
