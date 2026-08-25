module uart_msg_handler (
	reset_n,
	sys_clk,
	tx_data_avail,
	tx_rd,
	tx_data,
	rx_ready,
	rx_wr,
	rx_data,
	reg_addr,
	reg_wr,
	reg_wdata,
	reg_req,
	reg_ack,
	reg_rdata
);
	input reset_n;
	input sys_clk;
	output reg tx_data_avail;
	output reg [7:0] tx_data;
	input tx_rd;
	output wire rx_ready;
	input [7:0] rx_data;
	input rx_wr;
	output reg [15:0] reg_addr;
	output reg [31:0] reg_wdata;
	output reg reg_req;
	output reg reg_wr;
	input reg_ack;
	input [31:0] reg_rdata;
	reg [127:0] TxMsgBuf;
	reg [4:0] TxMsgSize;
	reg [4:0] RxMsgCnt;
	reg [3:0] State;
	reg [3:0] NextState;
	reg [15:0] cmd;
	assign rx_ready = 1;
	function [3:0] char2hex;
		input [7:0] data_in;
		case (data_in)
			8'h30: char2hex = 4'h0;
			8'h31: char2hex = 4'h1;
			8'h32: char2hex = 4'h2;
			8'h33: char2hex = 4'h3;
			8'h34: char2hex = 4'h4;
			8'h35: char2hex = 4'h5;
			8'h36: char2hex = 4'h6;
			8'h37: char2hex = 4'h7;
			8'h38: char2hex = 4'h8;
			8'h39: char2hex = 4'h9;
			8'h41: char2hex = 4'ha;
			8'h42: char2hex = 4'hb;
			8'h43: char2hex = 4'hc;
			8'h44: char2hex = 4'hd;
			8'h45: char2hex = 4'he;
			8'h46: char2hex = 4'hf;
			8'h61: char2hex = 4'ha;
			8'h62: char2hex = 4'hb;
			8'h63: char2hex = 4'hc;
			8'h64: char2hex = 4'hd;
			8'h65: char2hex = 4'he;
			8'h66: char2hex = 4'hf;
			default: char2hex = 4'hf;
		endcase
	endfunction
	function [7:0] hex2char;
		input [3:0] data_in;
		case (data_in)
			4'h0: hex2char = 8'h30;
			4'h1: hex2char = 8'h31;
			4'h2: hex2char = 8'h32;
			4'h3: hex2char = 8'h33;
			4'h4: hex2char = 8'h34;
			4'h5: hex2char = 8'h35;
			4'h6: hex2char = 8'h36;
			4'h7: hex2char = 8'h37;
			4'h8: hex2char = 8'h38;
			4'h9: hex2char = 8'h39;
			4'ha: hex2char = 8'h41;
			4'hb: hex2char = 8'h42;
			4'hc: hex2char = 8'h43;
			4'hd: hex2char = 8'h44;
			4'he: hex2char = 8'h45;
			4'hf: hex2char = 8'h46;
		endcase
	endfunction
	always @(negedge reset_n or posedge sys_clk)
		if (reset_n == 1'b0) begin
			tx_data_avail <= 0;
			reg_req <= 0;
			State <= 4'h0;
			NextState <= 4'h0;
		end
		else
			case (State)
				4'h0: begin
					TxMsgBuf <= "Command Format:\n";
					TxMsgSize <= 16;
					tx_data_avail <= 0;
					State <= 4'ha;
					NextState <= 4'h1;
				end
				4'h1: begin
					TxMsgBuf <= "wm <ad> <data>\n ";
					TxMsgSize <= 15;
					tx_data_avail <= 0;
					State <= 4'ha;
					NextState <= 4'h2;
				end
				4'h2: begin
					TxMsgBuf <= "rm <ad>\n>>      ";
					TxMsgSize <= 10;
					tx_data_avail <= 0;
					RxMsgCnt <= 0;
					State <= 4'ha;
					NextState <= 4'h3;
				end
				4'h3:
					if (rx_wr == 1) begin
						if ((RxMsgCnt == 0) && (rx_data == 8'h20))
							;
						else if ((RxMsgCnt > 0) && (rx_data == 8'h20)) begin
							if (cmd == 16'h776d) begin
								RxMsgCnt <= 0;
								reg_addr <= 0;
								reg_wdata <= 0;
								State <= 4'h4;
							end
							else if (cmd == 16'h726d) begin
								reg_addr <= 0;
								RxMsgCnt <= 0;
								State <= 4'h7;
							end
							else
								State <= 4'h0;
						end
						else if (rx_data == 8'h0a)
							State <= 4'h0;
						else begin
							cmd <= (cmd << 8) | rx_data;
							RxMsgCnt <= RxMsgCnt + 1;
						end
					end
				4'h4:
					if (rx_wr == 1) begin
						if ((RxMsgCnt == 0) && (rx_data == 8'h20))
							;
						else if ((RxMsgCnt > 0) && (rx_data == 8'h20))
							State <= 4'h5;
						else if (rx_data == 8'h0a)
							State <= 4'h0;
						else begin
							reg_addr <= (reg_addr << 4) | char2hex(rx_data);
							RxMsgCnt <= RxMsgCnt + 1;
						end
					end
				4'h5:
					if (rx_wr == 1) begin
						if (rx_data == 8'h20)
							;
						else if (rx_data == 8'h0a) begin
							State <= 4'h6;
							reg_wr <= 1'b1;
							reg_req <= 1'b1;
						end
						else
							reg_wdata <= (reg_wdata << 4) | char2hex(rx_data);
					end
				4'h6:
					if (reg_ack) begin
						reg_req <= 1'b0;
						TxMsgBuf <= "cmd success\n>>  ";
						TxMsgSize <= 14;
						tx_data_avail <= 0;
						State <= 4'ha;
						NextState <= 4'h3;
					end
				4'h7:
					if (rx_wr == 1) begin
						if (rx_data == 8'h20)
							;
						else if (rx_data == 8'h0a) begin
							State <= 4'h8;
							reg_wr <= 1'b0;
							reg_req <= 1'b1;
						end
						else begin
							reg_addr <= (reg_addr << 4) | char2hex(rx_data);
							RxMsgCnt <= RxMsgCnt + 1;
						end
					end
				4'h8:
					if (reg_ack) begin
						reg_req <= 1'b0;
						TxMsgBuf <= "Response:       ";
						TxMsgSize <= 10;
						tx_data_avail <= 0;
						State <= 4'ha;
						NextState <= 4'h9;
					end
				4'h9: begin
					TxMsgBuf[127:120] <= hex2char(reg_rdata[31:28]);
					TxMsgBuf[119:112] <= hex2char(reg_rdata[27:24]);
					TxMsgBuf[111:104] <= hex2char(reg_rdata[23:20]);
					TxMsgBuf[103:96] <= hex2char(reg_rdata[19:16]);
					TxMsgBuf[95:88] <= hex2char(reg_rdata[15:12]);
					TxMsgBuf[87:80] <= hex2char(reg_rdata[11:8]);
					TxMsgBuf[79:72] <= hex2char(reg_rdata[7:4]);
					TxMsgBuf[71:64] <= hex2char(reg_rdata[3:0]);
					TxMsgBuf[63:56] <= "\n";
					TxMsgSize <= 9;
					tx_data_avail <= 0;
					State <= 4'ha;
					NextState <= 4'h3;
				end
				4'ha: begin
					tx_data_avail <= 1;
					tx_data <= TxMsgBuf[127:120];
					if (TxMsgSize == 0) begin
						tx_data_avail <= 0;
						State <= NextState;
					end
					else if (tx_rd) begin
						TxMsgBuf <= TxMsgBuf << 8;
						TxMsgSize <= TxMsgSize - 1;
					end
				end
			endcase
endmodule
