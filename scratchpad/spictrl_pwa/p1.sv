// @mununu_guarantee nu Z. (((SDInitReq == 0 or txDataWen == 0) and (SDInitReq == 0 or readWriteSDBlockReq == 0) and (txDataWen == 0 or readWriteSDBlockReq == 0)) and [] Z)
module spiCtrl (
	clk,
	readWriteSDBlockRdy,
	readWriteSDBlockReq,
	rst,
	rxDataRdy,
	rxDataRdyClr,
	SDInitRdy,
	SDInitReq,
	spiCS_n,
	spiTransCtrl,
	spiTransSts,
	spiTransType,
	txDataWen
);
	input wire clk;
	input wire readWriteSDBlockRdy;
	input wire rst;
	input wire rxDataRdy;
	input wire SDInitRdy;
	input wire spiTransCtrl;
	input wire [1:0] spiTransType;
	output reg [1:0] readWriteSDBlockReq;
	output reg rxDataRdyClr;
	output reg SDInitReq;
	output reg spiCS_n;
	output reg spiTransSts;
	output reg txDataWen;
	reg [1:0] next_readWriteSDBlockReq;
	reg next_rxDataRdyClr;
	reg next_SDInitReq;
	reg next_spiCS_n;
	reg next_spiTransSts;
	reg next_txDataWen;
	reg [2:0] CurrState_spiCtrlSt;
	reg [2:0] NextState_spiCtrlSt;
	always @(spiTransCtrl or rxDataRdy or spiTransType or SDInitRdy or readWriteSDBlockRdy or readWriteSDBlockReq or txDataWen or SDInitReq or rxDataRdyClr or spiTransSts or spiCS_n or CurrState_spiCtrlSt) begin
		NextState_spiCtrlSt <= CurrState_spiCtrlSt;
		next_readWriteSDBlockReq <= readWriteSDBlockReq;
		next_txDataWen <= txDataWen;
		next_SDInitReq <= SDInitReq;
		next_rxDataRdyClr <= rxDataRdyClr;
		next_spiTransSts <= spiTransSts;
		next_spiCS_n <= spiCS_n;
		case (CurrState_spiCtrlSt)
			3'b000: begin
				next_readWriteSDBlockReq <= 2'b00;
				next_txDataWen <= 1'b0;
				next_SDInitReq <= 1'b0;
				next_rxDataRdyClr <= 1'b0;
				next_spiTransSts <= 1'b0;
				next_spiCS_n <= 1'b1;
				NextState_spiCtrlSt <= 3'b001;
			end
			3'b001: begin
				next_rxDataRdyClr <= 1'b0;
				next_spiTransSts <= 1'b0;
				if ((spiTransCtrl == 1'b1) && (spiTransType == 2'b01)) begin
					NextState_spiCtrlSt <= 3'b100;
					next_spiTransSts <= 1'b1;
					next_SDInitReq <= 1'b1;
				end
				else if ((spiTransCtrl == 1'b1) && (spiTransType == 2'b11)) begin
					NextState_spiCtrlSt <= 3'b110;
					next_spiTransSts <= 1'b1;
					next_readWriteSDBlockReq <= 2'b01;
				end
				else if ((spiTransCtrl == 1'b1) && (spiTransType == 2'b10)) begin
					NextState_spiCtrlSt <= 3'b110;
					next_spiTransSts <= 1'b1;
					next_readWriteSDBlockReq <= 2'b10;
				end
				else if ((spiTransCtrl == 1'b1) && (spiTransType == 2'b00)) begin
					NextState_spiCtrlSt <= 3'b011;
					next_spiTransSts <= 1'b1;
					next_txDataWen <= 1'b1;
					next_spiCS_n <= 1'b0;
				end
			end
			3'b010:
				if (rxDataRdy == 1'b1) begin
					NextState_spiCtrlSt <= 3'b001;
					next_rxDataRdyClr <= 1'b1;
					next_spiCS_n <= 1'b1;
				end
			3'b011: begin
				next_txDataWen <= 1'b0;
				NextState_spiCtrlSt <= 3'b010;
			end
			3'b100: begin
				next_SDInitReq <= 1'b0;
				NextState_spiCtrlSt <= 3'b101;
			end
			3'b101:
				if (SDInitRdy == 1'b1)
					NextState_spiCtrlSt <= 3'b001;
			3'b110: begin
				next_readWriteSDBlockReq <= 2'b00;
				NextState_spiCtrlSt <= 3'b111;
			end
			3'b111:
				if (readWriteSDBlockRdy == 1'b1)
					NextState_spiCtrlSt <= 3'b001;
		endcase
	end
	always @(posedge clk)
		if (rst == 1'b1)
			CurrState_spiCtrlSt <= 3'b000;
		else
			CurrState_spiCtrlSt <= NextState_spiCtrlSt;
	always @(posedge clk)
		if (rst == 1'b1) begin
			readWriteSDBlockReq <= 2'b00;
			txDataWen <= 1'b0;
			SDInitReq <= 1'b0;
			rxDataRdyClr <= 1'b0;
			spiTransSts <= 1'b0;
			spiCS_n <= 1'b1;
		end
		else begin
			readWriteSDBlockReq <= next_readWriteSDBlockReq;
			txDataWen <= next_txDataWen;
			SDInitReq <= next_SDInitReq;
			rxDataRdyClr <= next_rxDataRdyClr;
			spiTransSts <= next_spiTransSts;
			spiCS_n <= next_spiCS_n;
		end
endmodule
