module i2cSlaveTop (
	clk,
	rst,
	sda,
	scl,
	myReg0
);
	input clk;
	input rst;
	inout sda;
	input scl;
	output wire [7:0] myReg0;
	i2cSlave u_i2cSlave(
		.clk(clk),
		.rst(rst),
		.sda(sda),
		.scl(scl),
		.myReg0(myReg0),
		.myReg1(),
		.myReg2(),
		.myReg3(),
		.myReg4(8'h12),
		.myReg5(8'h34),
		.myReg6(8'h56),
		.myReg7(8'h78)
	);
endmodule
module i2cSlave (
	clk,
	rst,
	sda,
	scl,
	myReg0,
	myReg1,
	myReg2,
	myReg3,
	myReg4,
	myReg5,
	myReg6,
	myReg7
);
	input clk;
	input rst;
	inout sda;
	input scl;
	output wire [7:0] myReg0;
	output wire [7:0] myReg1;
	output wire [7:0] myReg2;
	output wire [7:0] myReg3;
	input [7:0] myReg4;
	input [7:0] myReg5;
	input [7:0] myReg6;
	input [7:0] myReg7;
	reg sdaDeb;
	reg sclDeb;
	reg [9:0] sdaPipe;
	reg [9:0] sclPipe;
	reg [9:0] sclDelayed;
	reg [3:0] sdaDelayed;
	reg [1:0] startStopDetState;
	wire clearStartStopDet;
	wire sdaOut;
	wire sdaIn;
	wire [7:0] regAddr;
	wire [7:0] dataToRegIF;
	wire writeEn;
	wire [7:0] dataFromRegIF;
	reg [1:0] rstPipe;
	wire rstSyncToClk;
	reg startEdgeDet;
	assign sda = (sdaOut == 1'b0 ? 1'b0 : 1'bz);
	assign sdaIn = sda;
	always @(posedge clk)
		if (rst == 1'b1)
			rstPipe <= 2'b11;
		else
			rstPipe <= {rstPipe[0], 1'b0};
	assign rstSyncToClk = rstPipe[1];
	always @(posedge clk)
		if (rstSyncToClk == 1'b1) begin
			sdaPipe <= {10 {1'b1}};
			sdaDeb <= 1'b1;
			sclPipe <= {10 {1'b1}};
			sclDeb <= 1'b1;
		end
		else begin
			sdaPipe <= {sdaPipe[8:0], sdaIn};
			sclPipe <= {sclPipe[8:0], scl};
			if (&sclPipe[9:1] == 1'b1)
				sclDeb <= 1'b1;
			else if (|sclPipe[9:1] == 1'b0)
				sclDeb <= 1'b0;
			if (&sdaPipe[9:1] == 1'b1)
				sdaDeb <= 1'b1;
			else if (|sdaPipe[9:1] == 1'b0)
				sdaDeb <= 1'b0;
		end
	always @(posedge clk)
		if (rstSyncToClk == 1'b1) begin
			sclDelayed <= {10 {1'b1}};
			sdaDelayed <= {4 {1'b1}};
		end
		else begin
			sclDelayed <= {sclDelayed[8:0], sclDeb};
			sdaDelayed <= {sdaDelayed[2:0], sdaDeb};
		end
	always @(posedge clk)
		if (rstSyncToClk == 1'b1) begin
			startStopDetState <= 2'b00;
			startEdgeDet <= 1'b0;
		end
		else begin
			if (((sclDeb == 1'b1) && (sdaDelayed[2] == 1'b0)) && (sdaDelayed[3] == 1'b1))
				startEdgeDet <= 1'b1;
			else
				startEdgeDet <= 1'b0;
			if (clearStartStopDet == 1'b1)
				startStopDetState <= 2'b00;
			else if (sclDeb == 1'b1) begin
				if ((sdaDelayed[2] == 1'b1) && (sdaDelayed[3] == 1'b0))
					startStopDetState <= 2'b10;
				else if ((sdaDelayed[2] == 1'b0) && (sdaDelayed[3] == 1'b1))
					startStopDetState <= 2'b01;
			end
		end
	registerInterface u_registerInterface(
		.clk(clk),
		.addr(regAddr),
		.dataIn(dataToRegIF),
		.writeEn(writeEn),
		.dataOut(dataFromRegIF),
		.myReg0(myReg0),
		.myReg1(myReg1),
		.myReg2(myReg2),
		.myReg3(myReg3),
		.myReg4(myReg4),
		.myReg5(myReg5),
		.myReg6(myReg6),
		.myReg7(myReg7)
	);
	serialInterface u_serialInterface(
		.clk(clk),
		.rst(rstSyncToClk | startEdgeDet),
		.dataIn(dataFromRegIF),
		.dataOut(dataToRegIF),
		.writeEn(writeEn),
		.regAddr(regAddr),
		.scl(sclDelayed[9]),
		.sdaIn(sdaDeb),
		.sdaOut(sdaOut),
		.startStopDetState(startStopDetState),
		.clearStartStopDet(clearStartStopDet)
	);
endmodule
module registerInterface (
	clk,
	addr,
	dataIn,
	writeEn,
	dataOut,
	myReg0,
	myReg1,
	myReg2,
	myReg3,
	myReg4,
	myReg5,
	myReg6,
	myReg7
);
	input clk;
	input [7:0] addr;
	input [7:0] dataIn;
	input writeEn;
	output reg [7:0] dataOut;
	output reg [7:0] myReg0;
	output reg [7:0] myReg1;
	output reg [7:0] myReg2;
	output reg [7:0] myReg3;
	input [7:0] myReg4;
	input [7:0] myReg5;
	input [7:0] myReg6;
	input [7:0] myReg7;
	always @(posedge clk)
		case (addr)
			8'h00: dataOut <= myReg0;
			8'h01: dataOut <= myReg1;
			8'h02: dataOut <= myReg2;
			8'h03: dataOut <= myReg3;
			8'h04: dataOut <= myReg4;
			8'h05: dataOut <= myReg5;
			8'h06: dataOut <= myReg6;
			8'h07: dataOut <= myReg7;
			default: dataOut <= 8'h00;
		endcase
	always @(posedge clk)
		if (writeEn == 1'b1)
			case (addr)
				8'h00: myReg0 <= dataIn;
				8'h01: myReg1 <= dataIn;
				8'h02: myReg2 <= dataIn;
				8'h03: myReg3 <= dataIn;
			endcase
endmodule
module serialInterface (
	clearStartStopDet,
	clk,
	dataIn,
	dataOut,
	regAddr,
	rst,
	scl,
	sdaIn,
	sdaOut,
	startStopDetState,
	writeEn
);
	input wire clk;
	input wire [7:0] dataIn;
	input wire rst;
	input wire scl;
	input wire sdaIn;
	input wire [1:0] startStopDetState;
	output reg clearStartStopDet;
	output reg [7:0] dataOut;
	output reg [7:0] regAddr;
	output reg sdaOut;
	output reg writeEn;
	reg next_clearStartStopDet;
	reg [7:0] next_dataOut;
	reg [7:0] next_regAddr;
	reg next_sdaOut;
	reg next_writeEn;
	reg [2:0] bitCnt;
	reg [2:0] next_bitCnt;
	reg [7:0] rxData;
	reg [7:0] next_rxData;
	reg [1:0] streamSt;
	reg [1:0] next_streamSt;
	reg [7:0] txData;
	reg [7:0] next_txData;
	reg [3:0] CurrState_SISt;
	reg [3:0] NextState_SISt;
	always @(startStopDetState or streamSt or scl or txData or bitCnt or rxData or sdaIn or regAddr or dataIn or sdaOut or writeEn or dataOut or clearStartStopDet or CurrState_SISt) begin
		NextState_SISt <= CurrState_SISt;
		next_streamSt <= streamSt;
		next_txData <= txData;
		next_rxData <= rxData;
		next_sdaOut <= sdaOut;
		next_writeEn <= writeEn;
		next_dataOut <= dataOut;
		next_bitCnt <= bitCnt;
		next_clearStartStopDet <= clearStartStopDet;
		next_regAddr <= regAddr;
		case (CurrState_SISt)
			4'b0000: begin
				next_streamSt <= 2'b00;
				next_txData <= 8'h00;
				next_rxData <= 8'h00;
				next_sdaOut <= 1'b1;
				next_writeEn <= 1'b0;
				next_dataOut <= 8'h00;
				next_bitCnt <= 3'b000;
				next_clearStartStopDet <= 1'b0;
				NextState_SISt <= 4'b0001;
			end
			4'b0001:
				if (streamSt == 2'b01) begin
					NextState_SISt <= 4'b0010;
					next_txData <= dataIn;
					next_regAddr <= regAddr + 1'b1;
					next_bitCnt <= 3'b001;
				end
				else begin
					NextState_SISt <= 4'b1000;
					next_rxData <= 8'h00;
				end
			4'b0010:
				if (scl == 1'b0) begin
					NextState_SISt <= 4'b0011;
					next_sdaOut <= txData[7];
					next_txData <= {txData[6:0], 1'b0};
				end
			4'b0011:
				if (scl == 1'b1)
					NextState_SISt <= 4'b0100;
			4'b0100:
				if (bitCnt == 3'b000)
					NextState_SISt <= 4'b0101;
				else begin
					NextState_SISt <= 4'b0010;
					next_bitCnt <= bitCnt + 1'b1;
				end
			4'b0101:
				if (scl == 1'b0) begin
					NextState_SISt <= 4'b0110;
					next_sdaOut <= 1'b1;
				end
			4'b0110:
				if (scl == 1'b1) begin
					NextState_SISt <= 4'b0001;
					if (sdaIn == 1'b1)
						next_streamSt <= 2'b00;
				end
			4'b0111:
				if ((scl == 1'b0) && ((startStopDetState == 2'b10) || ((streamSt == 2'b00) && (startStopDetState == 2'b00)))) begin
					NextState_SISt <= 4'b1111;
					case (startStopDetState)
						2'b00: next_bitCnt <= bitCnt + 1'b1;
						2'b01: begin
							next_streamSt <= 2'b00;
							next_rxData <= 8'h00;
						end
						default:
							;
					endcase
					next_streamSt <= 2'b00;
					next_clearStartStopDet <= 1'b1;
				end
				else if (scl == 1'b0) begin
					NextState_SISt <= 4'b1011;
					case (startStopDetState)
						2'b00: next_bitCnt <= bitCnt + 1'b1;
						2'b01: begin
							next_streamSt <= 2'b00;
							next_rxData <= 8'h00;
						end
						default:
							;
					endcase
				end
			4'b1000:
				if (scl == 1'b1) begin
					NextState_SISt <= 4'b0111;
					next_rxData <= {rxData[6:0], sdaIn};
					next_bitCnt <= 3'b000;
				end
			4'b1001:
				if (bitCnt == 3'b111) begin
					NextState_SISt <= 4'b1110;
					next_sdaOut <= 1'b0;
					case (streamSt)
						2'b00:
							if ((rxData[7:1] == 7'h3c) && (startStopDetState == 2'b01)) begin
								if (rxData[0] == 1'b1)
									next_streamSt <= 2'b01;
								else
									next_streamSt <= 2'b10;
							end
							else
								next_sdaOut <= 1'b1;
						2'b10: begin
							next_streamSt <= 2'b11;
							next_regAddr <= rxData;
						end
						2'b11: begin
							next_dataOut <= rxData;
							next_writeEn <= 1'b1;
						end
						default: next_streamSt <= streamSt;
					endcase
				end
				else begin
					NextState_SISt <= 4'b1011;
					next_bitCnt <= bitCnt + 1'b1;
				end
			4'b1010:
				if (scl == 1'b0)
					NextState_SISt <= 4'b1001;
			4'b1011:
				if (scl == 1'b1) begin
					NextState_SISt <= 4'b1010;
					next_rxData <= {rxData[6:0], sdaIn};
				end
			4'b1100:
				if (scl == 1'b0) begin
					NextState_SISt <= 4'b0001;
					next_sdaOut <= 1'b1;
				end
			4'b1101: begin
				next_clearStartStopDet <= 1'b0;
				if (scl == 1'b1)
					NextState_SISt <= 4'b1100;
			end
			4'b1110: begin
				if (writeEn == 1'b1)
					next_regAddr <= regAddr + 1'b1;
				next_writeEn <= 1'b0;
				next_clearStartStopDet <= 1'b1;
				NextState_SISt <= 4'b1101;
			end
			4'b1111: begin
				next_clearStartStopDet <= 1'b0;
				NextState_SISt <= 4'b0001;
			end
		endcase
	end
	always @(posedge clk)
		if (rst == 1'b1)
			CurrState_SISt <= 4'b0000;
		else
			CurrState_SISt <= NextState_SISt;
	always @(posedge clk)
		if (rst == 1'b1) begin
			sdaOut <= 1'b1;
			writeEn <= 1'b0;
			dataOut <= 8'h00;
			clearStartStopDet <= 1'b0;
			streamSt <= 2'b00;
			txData <= 8'h00;
			rxData <= 8'h00;
			bitCnt <= 3'b000;
		end
		else begin
			sdaOut <= next_sdaOut;
			writeEn <= next_writeEn;
			dataOut <= next_dataOut;
			clearStartStopDet <= next_clearStartStopDet;
			regAddr <= next_regAddr;
			streamSt <= next_streamSt;
			txData <= next_txData;
			rxData <= next_rxData;
			bitCnt <= next_bitCnt;
		end
endmodule
