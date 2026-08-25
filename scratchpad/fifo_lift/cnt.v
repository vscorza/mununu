module prim_fifo_sync_cnt (
	clk_i,
	rst_ni,
	clr_i,
	incr_wptr_i,
	incr_rptr_i,
	wptr_o,
	rptr_o,
	full_o,
	empty_o,
	depth_o,
	err_o
);
	parameter [31:0] Depth = 4;
	parameter [0:0] Secure = 1'b0;
	parameter [0:0] NeverClears = 1'b0;
	function automatic integer prim_util_pkg_vbits;
		input integer value;
		prim_util_pkg_vbits = (value == 1 ? 1 : $clog2(value));
	endfunction
	localparam [31:0] PtrW = prim_util_pkg_vbits(Depth);
	localparam [31:0] DepthW = prim_util_pkg_vbits(Depth + 1);
	input clk_i;
	input rst_ni;
	input clr_i;
	input incr_wptr_i;
	input incr_rptr_i;
	output wire [PtrW - 1:0] wptr_o;
	output wire [PtrW - 1:0] rptr_o;
	output wire full_o;
	output wire empty_o;
	output wire [DepthW - 1:0] depth_o;
	output wire err_o;
	localparam [31:0] WrapPtrW = PtrW + 1;
	reg [WrapPtrW - 1:0] wptr_wrap_cnt_q;
	wire [WrapPtrW - 1:0] wptr_wrap_set_cnt;
	reg [WrapPtrW - 1:0] rptr_wrap_cnt_q;
	wire [WrapPtrW - 1:0] rptr_wrap_set_cnt;
	assign wptr_o = wptr_wrap_cnt_q[PtrW - 1:0];
	assign rptr_o = rptr_wrap_cnt_q[PtrW - 1:0];
	wire wptr_wrap_msb;
	wire rptr_wrap_msb;
	assign wptr_wrap_msb = wptr_wrap_cnt_q[WrapPtrW - 1];
	assign rptr_wrap_msb = rptr_wrap_cnt_q[WrapPtrW - 1];
	wire wptr_wrap_set;
	wire rptr_wrap_set;
	function automatic [PtrW - 1:0] sv2v_cast_09B00;
		input reg [PtrW - 1:0] inp;
		sv2v_cast_09B00 = inp;
	endfunction
	assign wptr_wrap_set = incr_wptr_i & (wptr_o == sv2v_cast_09B00(Depth - 1));
	assign rptr_wrap_set = incr_rptr_i & (rptr_o == sv2v_cast_09B00(Depth - 1));
	assign wptr_wrap_set_cnt = {~wptr_wrap_msb, {WrapPtrW - 1 {1'b0}}};
	assign rptr_wrap_set_cnt = {~rptr_wrap_msb, {WrapPtrW - 1 {1'b0}}};
	assign full_o = wptr_wrap_cnt_q == (rptr_wrap_cnt_q ^ {1'b1, {WrapPtrW - 1 {1'b0}}});
	assign empty_o = wptr_wrap_cnt_q == rptr_wrap_cnt_q;
	function automatic [DepthW - 1:0] sv2v_cast_2DA09;
		input reg [DepthW - 1:0] inp;
		sv2v_cast_2DA09 = inp;
	endfunction
	assign depth_o = (full_o ? sv2v_cast_2DA09(Depth) : (wptr_wrap_msb == rptr_wrap_msb ? sv2v_cast_2DA09(wptr_o) - sv2v_cast_2DA09(rptr_o) : (sv2v_cast_2DA09(Depth) - sv2v_cast_2DA09(rptr_o)) + sv2v_cast_2DA09(wptr_o)));
	function automatic [WrapPtrW - 1:0] sv2v_cast_00CF8;
		input reg [WrapPtrW - 1:0] inp;
		sv2v_cast_00CF8 = inp;
	endfunction
	generate
		if (Secure) begin : gen_secure_ptrs
			wire wptr_err;
			prim_count #(
				.Width(WrapPtrW),
				.PossibleActions(((NeverClears ? 0 : 4'h1) | 4'h2) | 4'h4)
			) u_wptr(
				.clk_i(clk_i),
				.rst_ni(rst_ni),
				.clr_i(clr_i),
				.set_i(wptr_wrap_set),
				.set_cnt_i(wptr_wrap_set_cnt),
				.incr_en_i(incr_wptr_i),
				.decr_en_i(1'b0),
				.step_i(sv2v_cast_00CF8(1'b1)),
				.commit_i(1'b1),
				.cnt_o(wptr_wrap_cnt_q),
				.cnt_after_commit_o(),
				.err_o(wptr_err)
			);
			wire rptr_err;
			prim_count #(
				.Width(WrapPtrW),
				.PossibleActions(((NeverClears ? 0 : 4'h1) | 4'h2) | 4'h4)
			) u_rptr(
				.clk_i(clk_i),
				.rst_ni(rst_ni),
				.clr_i(clr_i),
				.set_i(rptr_wrap_set),
				.set_cnt_i(rptr_wrap_set_cnt),
				.incr_en_i(incr_rptr_i),
				.decr_en_i(1'b0),
				.step_i(sv2v_cast_00CF8(1'b1)),
				.commit_i(1'b1),
				.cnt_o(rptr_wrap_cnt_q),
				.cnt_after_commit_o(),
				.err_o(rptr_err)
			);
			assign err_o = wptr_err | rptr_err;
		end
		else begin : gen_normal_ptrs
			always @(posedge clk_i or negedge rst_ni)
				if (!rst_ni)
					wptr_wrap_cnt_q <= {WrapPtrW {1'b0}};
				else if (clr_i)
					wptr_wrap_cnt_q <= {WrapPtrW {1'b0}};
				else if (wptr_wrap_set)
					wptr_wrap_cnt_q <= wptr_wrap_set_cnt;
				else if (incr_wptr_i)
					wptr_wrap_cnt_q <= wptr_wrap_cnt_q + {{WrapPtrW - 1 {1'b0}}, 1'b1};
			always @(posedge clk_i or negedge rst_ni)
				if (!rst_ni)
					rptr_wrap_cnt_q <= {WrapPtrW {1'b0}};
				else if (clr_i)
					rptr_wrap_cnt_q <= {WrapPtrW {1'b0}};
				else if (rptr_wrap_set)
					rptr_wrap_cnt_q <= rptr_wrap_set_cnt;
				else if (incr_rptr_i)
					rptr_wrap_cnt_q <= rptr_wrap_cnt_q + {{WrapPtrW - 1 {1'b0}}, 1'b1};
			assign err_o = 1'sb0;
		end
	endgenerate
endmodule
