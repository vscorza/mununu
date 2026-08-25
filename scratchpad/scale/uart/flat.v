module rtfSimpleUart (
	rst_i,
	clk_i,
	cyc_i,
	stb_i,
	we_i,
	adr_i,
	dat_i,
	dat_o,
	ack_o,
	vol_o,
	irq_o,
	cts_ni,
	rts_no,
	dsr_ni,
	dcd_ni,
	dtr_no,
	rxd_i,
	txd_o,
	data_present_o
);
	input rst_i;
	input clk_i;
	input cyc_i;
	input stb_i;
	input we_i;
	input [31:0] adr_i;
	input [7:0] dat_i;
	output reg [7:0] dat_o;
	output wire ack_o;
	output wire vol_o;
	output wire irq_o;
	input cts_ni;
	output reg rts_no;
	input dsr_ni;
	input dcd_ni;
	output reg dtr_no;
	input rxd_i;
	output wire txd_o;
	output wire data_present_o;
	parameter pClkFreq = 20000000;
	parameter pBaud = 19200;
	parameter pClkMul = (4096 * pBaud) / (pClkFreq / 65536);
	parameter pRts = 1;
	parameter pDtr = 1;
	wire cs = (cyc_i && stb_i) && (adr_i[31:4] == 28'hffdc0a0);
	assign ack_o = cs;
	assign vol_o = cs && (adr_i[3:2] == 2'b00);
	reg [23:0] c;
	reg [23:0] ck_mul;
	wire tx_empty;
	wire baud16;
	reg rx_present_ie;
	reg tx_empty_ie;
	reg dcd_ie;
	reg hwfc;
	wire clear = ((cyc_i && stb_i) && we_i) && (adr_i == 4'd13);
	wire frame_err;
	wire over_run;
	reg [1:0] ctsx;
	reg [1:0] dcdx;
	reg [1:0] dsrx;
	wire dcd_chg = dcdx[1] ^ dcdx[0];
	wire rxIRQ = data_present_o & rx_present_ie;
	wire txIRQ = tx_empty & tx_empty_ie;
	wire msIRQ = dcd_chg & dcd_ie;
	assign irq_o = (rxIRQ | txIRQ) | msIRQ;
	wire [2:0] irqenc = (rxIRQ ? 1 : (txIRQ ? 3 : (msIRQ ? 4 : 0)));
	wire [7:0] rx_do;
	wire txrx = cs && (adr_i[3:0] == 4'd0);
	rtfSimpleUartRx uart_rx0(
		.rst_i(rst_i),
		.clk_i(clk_i),
		.cyc_i(cyc_i),
		.stb_i(stb_i),
		.cs_i(txrx),
		.we_i(we_i),
		.dat_o(rx_do),
		.baud16x_ce(baud16),
		.clear(clear),
		.rxd(rxd_i),
		.data_present(data_present_o),
		.frame_err(frame_err),
		.overrun(over_run)
	);
	rtfSimpleUartTx uart_tx0(
		.rst_i(rst_i),
		.clk_i(clk_i),
		.cyc_i(cyc_i),
		.stb_i(stb_i),
		.cs_i(txrx),
		.we_i(we_i),
		.dat_i(dat_i),
		.baud16x_ce(baud16),
		.cts(ctsx[1] | ~hwfc),
		.txd(txd_o),
		.empty(tx_empty)
	);
	always @(*)
		if (cs)
			case (adr_i[3:0])
				4'd2: dat_o <= {dcdx[1], 1'b0, dsrx[1], ctsx[1], dcd_chg, 3'b000};
				4'd3: dat_o <= {irq_o, 2'b00, irqenc, 2'b00};
				4'd1: dat_o <= {1'b0, tx_empty, tx_empty, 1'b0, frame_err, 1'b0, over_run, data_present_o};
				default: dat_o <= rx_do;
			endcase
		else
			dat_o <= 8'b00000000;
	always @(posedge clk_i)
		if (rst_i)
			c <= 0;
		else
			c <= c + ck_mul;
	edge_det ed0(
		.rst(rst_i),
		.clk(clk_i),
		.ce(1'b1),
		.i(c[23]),
		.pe(baud16),
		.ne(),
		.ee()
	);
	always @(posedge clk_i)
		if (rst_i) begin
			rts_no <= ~pRts;
			rx_present_ie <= 1'b0;
			tx_empty_ie <= 1'b0;
			dcd_ie <= 1'b0;
			hwfc <= 1'b1;
			dtr_no <= ~pDtr;
			ck_mul <= pClkMul;
		end
		else if (cs & we_i)
			case (adr_i)
				4'd4: begin
					rx_present_ie <= dat_i[0];
					tx_empty_ie <= dat_i[1];
					dcd_ie <= dat_i[3];
				end
				4'd6: begin
					dtr_no <= ~dat_i[0];
					rts_no <= ~dat_i[1];
				end
				4'd7: hwfc <= dat_i[0];
				4'd9: ck_mul[7:0] <= dat_i;
				4'd10: ck_mul[15:8] <= dat_i;
				4'd11: ck_mul[23:16] <= dat_i;
				default:
					;
			endcase
	always @(posedge clk_i) ctsx <= {ctsx[0], ~cts_ni};
	always @(posedge clk_i) dcdx <= {dcdx[0], ~dcd_ni};
	always @(posedge clk_i) dsrx <= {dsrx[0], ~dsr_ni};
endmodule
module edge_det (
	rst,
	clk,
	ce,
	i,
	pe,
	ne,
	ee
);
	input rst;
	input clk;
	input ce;
	input i;
	output wire pe;
	output wire ne;
	output wire ee;
	reg ed;
	always @(posedge clk)
		if (rst)
			ed <= 1'b0;
		else if (ce)
			ed <= i;
	assign pe = ~ed & i;
	assign ne = ed & ~i;
	assign ee = ed ^ i;
endmodule
module rtfSimpleUartRx (
	rst_i,
	clk_i,
	cyc_i,
	stb_i,
	ack_o,
	we_i,
	dat_o,
	cs_i,
	baud16x_ce,
	baud8x,
	clear,
	rxd,
	data_present,
	frame_err,
	overrun
);
	input rst_i;
	input clk_i;
	input cyc_i;
	input stb_i;
	output wire ack_o;
	input we_i;
	output wire [7:0] dat_o;
	input cs_i;
	input baud16x_ce;
	input tri0 baud8x;
	input clear;
	input rxd;
	output reg data_present;
	output reg frame_err;
	output reg overrun;
	parameter SamplerStyle = 0;
	reg [7:0] cnt;
	reg [9:0] rx_data;
	reg state;
	reg wf;
	reg [7:0] dat;
	wire isX8;
	buf (isX8, baud8x);
	reg modeX8;
	assign ack_o = (cyc_i & stb_i) & cs_i;
	assign dat_o = (ack_o ? dat : 8'b00000000);
	always @(posedge clk_i)
		if (wf)
			dat <= rx_data[8:1];
	always @(posedge clk_i)
		if (rst_i)
			data_present <= 0;
		else if (wf)
			data_present <= 1;
		else if (ack_o & ~we_i)
			data_present <= 0;
	reg [5:0] rxdd;
	reg rxdsmp;
	reg rdxstart;
	reg [1:0] rxdsum;
	always @(posedge clk_i)
		if (baud16x_ce) begin
			rxdd <= {rxdd[4:0], rxd};
			if (SamplerStyle == 0) begin
				rxdsmp <= rxdd[3];
				rdxstart <= rxdd[4] & ~rxdd[3];
			end
			else begin
				rxdsum[1] <= rxdsum[0];
				rxdsum[0] <= ({1'b0, rxdd[3]} + {1'b0, rxdd[4]}) + {1'b0, rxdd[5]};
				rxdsmp <= rxdsum[1];
				rdxstart <= (rxdsum[0] == 2'b00) & (rxdsum[1] == 2'b11);
			end
		end
	always @(posedge clk_i)
		if (rst_i) begin
			state <= 0;
			wf <= 1'b0;
			overrun <= 1'b0;
			frame_err <= 1'b0;
		end
		else begin
			wf <= 1'b0;
			if (clear) begin
				wf <= 1'b0;
				state <= 0;
				overrun <= 1'b0;
				frame_err <= 1'b0;
			end
			else if (baud16x_ce)
				case (state)
					0:
						if (rdxstart)
							state <= 1;
					1: begin
						if (cnt == 8'h97) begin
							frame_err <= ~rxdsmp;
							overrun <= data_present;
							if (!data_present)
								wf <= 1'b1;
							state <= 0;
						end
						if ((cnt == 8'h07) && rxdsmp)
							state <= 0;
						if (cnt[3:0] == 4'h7)
							rx_data <= {rxdsmp, rx_data[9:1]};
					end
				endcase
		end
	always @(posedge clk_i)
		if (baud16x_ce) begin
			if (state == 0) begin
				cnt <= modeX8;
				modeX8 <= isX8;
			end
			else begin
				cnt[7:1] <= cnt[7:1] + cnt[0];
				cnt[0] <= ~cnt[0] | modeX8;
			end
		end
endmodule
module rtfSimpleUartTx (
	rst_i,
	clk_i,
	cyc_i,
	stb_i,
	ack_o,
	we_i,
	dat_i,
	cs_i,
	baud16x_ce,
	baud8x,
	cts,
	txd,
	empty,
	txc
);
	input rst_i;
	input clk_i;
	input cyc_i;
	input stb_i;
	output wire ack_o;
	input we_i;
	input [7:0] dat_i;
	input cs_i;
	input baud16x_ce;
	input tri0 baud8x;
	input cts;
	output wire txd;
	output reg empty;
	output reg txc;
	reg [9:0] tx_data;
	reg [7:0] fdo;
	reg [7:0] cnt;
	reg rd;
	wire isX8;
	buf (isX8, baud8x);
	reg modeX8;
	assign ack_o = (cyc_i & stb_i) & cs_i;
	assign txd = tx_data[0];
	always @(posedge clk_i)
		if (ack_o & we_i)
			fdo <= dat_i;
	always @(posedge clk_i)
		if (rst_i)
			empty <= 1;
		else if (ack_o & we_i)
			empty <= 0;
		else if (rd)
			empty <= 1;
	always @(posedge clk_i)
		if (rst_i) begin
			cnt <= 8'h9f;
			rd <= 0;
			tx_data <= 10'h3ff;
			txc <= 1'b1;
			modeX8 <= 1'b0;
		end
		else begin
			rd <= 0;
			if (baud16x_ce) begin
				if (cnt == 8'h9f) begin
					modeX8 <= isX8;
					if (!empty && cts) begin
						tx_data <= {1'b1, fdo, 1'b0};
						rd <= 1;
						cnt <= modeX8;
						txc <= 1'b0;
					end
					else
						txc <= 1'b1;
				end
				else begin
					cnt[7:1] <= cnt[7:1] + cnt[0];
					cnt[0] <= ~cnt[0] | modeX8;
					if (cnt[3:0] == 4'hf)
						tx_data <= {1'b1, tx_data[9:1]};
				end
			end
		end
endmodule
