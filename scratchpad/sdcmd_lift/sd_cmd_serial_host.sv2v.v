module sd_cmd_serial_host (
	SD_CLK_IN,
	RST_IN,
	SETTING_IN,
	CMD_IN,
	REQ_IN,
	ACK_OUT,
	REQ_OUT,
	ACK_IN,
	CMD_OUT,
	STATUS,
	cmd_dat_i,
	cmd_out_o,
	cmd_oe_o,
	st_dat_t
);
	input wire SD_CLK_IN;
	input wire RST_IN;
	input wire [15:0] SETTING_IN;
	input wire [39:0] CMD_IN;
	input wire REQ_IN;
	input wire ACK_IN;
	input cmd_dat_i;
	output reg [39:0] CMD_OUT;
	output wire ACK_OUT;
	output reg REQ_OUT;
	output reg [7:0] STATUS;
	output reg cmd_oe_o;
	output reg cmd_out_o;
	output reg [1:0] st_dat_t;
	parameter SEND_SIZE = 48;
	parameter SIZE = 10;
	parameter CONTENT_SIZE = 40;
	parameter INIT = 10'b0000000001;
	parameter IDLE = 10'b0000000010;
	parameter WRITE_WR = 10'b0000000100;
	parameter DLY_WR = 10'b0000001000;
	parameter READ_WR = 10'b0000010000;
	parameter DLY_READ = 10'b0000100000;
	parameter ACK_WR = 10'b0001000000;
	parameter WRITE_WO = 10'b0010000000;
	parameter DLY_WO = 10'b0100000000;
	parameter ACK_WO = 10'b1000000000;
	parameter Read_Delay = 7;
	parameter EIGHT_PAD = 8;
	reg [6:0] Response_Size;
	reg [2:0] Delay_Cycler;
	reg [CONTENT_SIZE - 1:0] In_Buff;
	reg [39:0] Out_Buff;
	reg Write_Read;
	reg Write_Only;
	reg [4:0] word_select_counter;
	reg CRC_RST;
	reg [6:0] CRC_IN;
	wire [6:0] CRC_VAL;
	reg CRC_Enable;
	reg CRC_OUT;
	reg CRC_Check_On;
	reg Crc_Buffering;
	reg CRC_Valid;
	reg [7:0] Cmd_Cnt;
	reg [2:0] Delay_Cnt;
	reg [SIZE - 1:0] state;
	reg [SIZE - 1:0] next_state;
	reg block_write;
	reg block_read;
	reg [1:0] word_select;
	reg FSM_ACK;
	reg DECODER_ACK;
	reg q;
	reg Req_internal_in;
	reg q1;
	reg Ack_internal_in;
	sd_crc_7 CRC_7(
		.BITVAL(CRC_OUT),
		.Enable(CRC_Enable),
		.CLK(SD_CLK_IN),
		.RST(CRC_RST),
		.CRC(CRC_VAL)
	);
	always @(state or Delay_Cnt or Write_Read or Cmd_Cnt or Write_Only or Ack_internal_in or cmd_dat_i or Response_Size or Delay_Cycler) begin : FSM_COMBO
		next_state = 0;
		case (state)
			INIT:
				if (Cmd_Cnt >= 2)
					next_state = IDLE;
				else
					next_state = INIT;
			IDLE:
				if (Write_Read)
					next_state = WRITE_WR;
				else if (Write_Only)
					next_state = WRITE_WO;
				else
					next_state = IDLE;
			WRITE_WR:
				if (Cmd_Cnt >= (SEND_SIZE - 1))
					next_state = DLY_WR;
				else
					next_state = WRITE_WR;
			WRITE_WO:
				if (Cmd_Cnt >= (SEND_SIZE - 1))
					next_state = DLY_WO;
				else
					next_state = WRITE_WO;
			DLY_WR:
				if ((Delay_Cnt >= 2) && !cmd_dat_i)
					next_state = READ_WR;
				else
					next_state = DLY_WR;
			DLY_WO:
				if (Delay_Cnt >= Delay_Cycler)
					next_state = ACK_WO;
				else
					next_state = DLY_WO;
			READ_WR:
				if (Cmd_Cnt >= (Response_Size + EIGHT_PAD))
					next_state = DLY_READ;
				else
					next_state = READ_WR;
			ACK_WO: next_state = IDLE;
			DLY_READ:
				if (Ack_internal_in)
					next_state = ACK_WR;
				else
					next_state = DLY_READ;
			ACK_WR: next_state = IDLE;
			default: next_state = INIT;
		endcase
	end
	always @(posedge SD_CLK_IN or posedge RST_IN) begin : REQ_SYNC
		if (RST_IN) begin
			Req_internal_in <= 1'b0;
			q <= 1'b0;
		end
		else begin
			q <= REQ_IN;
			Req_internal_in <= q;
		end
	end
	always @(posedge SD_CLK_IN or posedge RST_IN) begin : ACK_SYNC
		if (RST_IN) begin
			Ack_internal_in <= 1'b0;
			q1 <= 1'b0;
		end
		else begin
			q1 <= ACK_IN;
			Ack_internal_in <= q1;
		end
	end
	always @(posedge SD_CLK_IN or posedge RST_IN) begin : COMMAND_DECODER
		if (RST_IN) begin
			Delay_Cycler <= 3'b000;
			Response_Size <= 7'b0000000;
			DECODER_ACK <= 1;
			Write_Read <= 1'b0;
			Write_Only <= 1'b0;
			CRC_Check_On <= 0;
			In_Buff <= 0;
			block_write <= 0;
			block_read <= 0;
			word_select <= 0;
		end
		else if (Req_internal_in == 1) begin
			Response_Size[6:0] <= SETTING_IN[6:0];
			CRC_Check_On <= SETTING_IN[7];
			Delay_Cycler[2:0] <= SETTING_IN[10:8];
			block_write <= SETTING_IN[11];
			block_read <= SETTING_IN[12];
			word_select <= SETTING_IN[14:13];
			In_Buff <= CMD_IN;
			DECODER_ACK <= 0;
			if (SETTING_IN[6:0] > 0) begin
				Write_Read <= 1'b1;
				Write_Only <= 1'b0;
			end
			else begin
				Write_Read <= 1'b0;
				Write_Only <= 1'b1;
			end
		end
		else begin
			Write_Read <= 1'b0;
			Write_Only <= 1'b0;
			DECODER_ACK <= 1;
		end
	end
	assign ACK_OUT = FSM_ACK & DECODER_ACK;
	always @(posedge SD_CLK_IN or posedge RST_IN) begin : FSM_SEQ
		if (RST_IN)
			state <= #(1) INIT;
		else
			state <= #(1) next_state;
	end
	always @(posedge SD_CLK_IN or posedge RST_IN) begin : FSM_OUT
		if (RST_IN) begin
			CRC_Enable = 0;
			word_select_counter <= 0;
			Delay_Cnt = 0;
			cmd_oe_o = 1;
			cmd_out_o = 1;
			Out_Buff = 0;
			FSM_ACK = 1;
			REQ_OUT = 0;
			CRC_RST = 1;
			CRC_OUT = 0;
			CRC_IN = 0;
			CMD_OUT = 0;
			Crc_Buffering = 0;
			STATUS = 0;
			CRC_Valid = 0;
			Cmd_Cnt = 0;
			st_dat_t <= 0;
		end
		else
			case (state)
				INIT: begin
					Cmd_Cnt = Cmd_Cnt + 1;
					cmd_oe_o = 1;
					cmd_out_o = 1;
				end
				IDLE: begin
					cmd_oe_o = 0;
					Delay_Cnt = 0;
					Cmd_Cnt = 0;
					CRC_RST = 1;
					CRC_Enable = 0;
					CMD_OUT = 0;
					st_dat_t <= 0;
					word_select_counter <= 0;
				end
				WRITE_WR: begin
					FSM_ACK = 0;
					CRC_RST = 0;
					CRC_Enable = 1;
					if (Cmd_Cnt == 0) begin
						STATUS = 16'b0000000000000001;
						REQ_OUT = 1;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					if (Crc_Buffering == 1) begin
						cmd_oe_o = 1;
						if ((SEND_SIZE - Cmd_Cnt) > 8) begin
							cmd_out_o = In_Buff[(CONTENT_SIZE - 1) - Cmd_Cnt];
							if ((SEND_SIZE - Cmd_Cnt) > 9)
								CRC_OUT = In_Buff[((CONTENT_SIZE - 1) - Cmd_Cnt) - 1];
							else
								CRC_Enable = 0;
						end
						else if (((SEND_SIZE - Cmd_Cnt) <= 8) && ((SEND_SIZE - Cmd_Cnt) >= 2)) begin
							CRC_Enable = 0;
							cmd_out_o = CRC_VAL[(SEND_SIZE - Cmd_Cnt) - 2];
							if (block_read & block_write)
								st_dat_t <= 2'b11;
							else if (block_read)
								st_dat_t <= 2'b10;
						end
						else
							cmd_out_o = 1'b1;
						Cmd_Cnt = Cmd_Cnt + 1;
					end
					else begin
						Crc_Buffering = 1;
						CRC_OUT = In_Buff[(CONTENT_SIZE - 1) - Cmd_Cnt];
					end
				end
				WRITE_WO: begin
					FSM_ACK = 0;
					CRC_RST = 0;
					CRC_Enable = 1;
					if (Cmd_Cnt == 0) begin
						STATUS[3:0] = 16'b0000000000000010;
						REQ_OUT = 1;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					if (Crc_Buffering == 1) begin
						cmd_oe_o = 1;
						if ((SEND_SIZE - Cmd_Cnt) > 8) begin
							cmd_out_o = In_Buff[(CONTENT_SIZE - 1) - Cmd_Cnt];
							if ((SEND_SIZE - Cmd_Cnt) > 9)
								CRC_OUT = In_Buff[((CONTENT_SIZE - 1) - Cmd_Cnt) - 1];
							else
								CRC_Enable = 0;
						end
						else if (((SEND_SIZE - Cmd_Cnt) <= 8) && ((SEND_SIZE - Cmd_Cnt) >= 2)) begin
							CRC_Enable = 0;
							cmd_out_o = CRC_VAL[(SEND_SIZE - Cmd_Cnt) - 2];
							if (block_read)
								st_dat_t <= 2'b10;
						end
						else
							cmd_out_o = 1'b1;
						Cmd_Cnt = Cmd_Cnt + 1;
					end
					else begin
						Crc_Buffering = 1;
						CRC_OUT = In_Buff[(CONTENT_SIZE - 1) - Cmd_Cnt];
					end
				end
				DLY_WR: begin
					if (Delay_Cnt == 0) begin
						STATUS[3:0] = 4'b0011;
						REQ_OUT = 1;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					CRC_Enable = 0;
					CRC_RST = 1;
					Cmd_Cnt = 1;
					cmd_oe_o = 0;
					if (Delay_Cnt < 3'b111)
						Delay_Cnt = Delay_Cnt + 1;
					Crc_Buffering = 0;
				end
				DLY_WO: begin
					if (Delay_Cnt == 0) begin
						STATUS[3:0] = 4'b0100;
						STATUS[5] = 0;
						STATUS[6] = 1;
						REQ_OUT = 1;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					CRC_Enable = 0;
					CRC_RST = 1;
					Cmd_Cnt = 0;
					cmd_oe_o = 0;
					Delay_Cnt = Delay_Cnt + 1;
					Crc_Buffering = 0;
				end
				READ_WR: begin
					Delay_Cnt = 0;
					CRC_RST = 0;
					CRC_Enable = 1;
					cmd_oe_o = 0;
					if (Cmd_Cnt == 1) begin
						STATUS[3:0] = 16'b0000000000000101;
						REQ_OUT = 1;
						Out_Buff[39] = 0;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					if (Cmd_Cnt < Response_Size) begin
						if (Cmd_Cnt < 8)
							Out_Buff[39 - Cmd_Cnt] = cmd_dat_i;
						else if (word_select == 2'b00) begin
							if (Cmd_Cnt < 40) begin
								word_select_counter <= word_select_counter + 1;
								Out_Buff[31 - word_select_counter] = cmd_dat_i;
							end
						end
						else if (word_select == 2'b01) begin
							if ((Cmd_Cnt >= 40) && (Cmd_Cnt < 72)) begin
								word_select_counter <= word_select_counter + 1;
								Out_Buff[31 - word_select_counter] = cmd_dat_i;
							end
						end
						else if (word_select == 2'b10) begin
							if ((Cmd_Cnt >= 72) && (Cmd_Cnt < 104)) begin
								word_select_counter <= word_select_counter + 1;
								Out_Buff[31 - word_select_counter] = cmd_dat_i;
							end
						end
						else if (word_select == 2'b11) begin
							if ((Cmd_Cnt >= 104) && (Cmd_Cnt < 128)) begin
								word_select_counter <= word_select_counter + 1;
								Out_Buff[31 - word_select_counter] = cmd_dat_i;
							end
						end
						CRC_OUT = cmd_dat_i;
					end
					else if ((Cmd_Cnt - Response_Size) <= 6) begin
						CRC_IN[(Response_Size + 6) - Cmd_Cnt] = cmd_dat_i;
						CRC_Enable = 0;
					end
					else begin
						if ((CRC_IN != CRC_VAL) && (CRC_Check_On == 1)) begin
							CRC_Valid = 0;
							CRC_Enable = 0;
						end
						else begin
							CRC_Valid = 1;
							CRC_Enable = 0;
						end
						if (block_read & block_write)
							st_dat_t <= 2'b11;
						else if (block_write)
							st_dat_t <= 2'b01;
					end
					Cmd_Cnt = Cmd_Cnt + 1;
				end
				DLY_READ: begin
					if (Delay_Cnt == 0) begin
						STATUS[3:0] = 4'b0110;
						STATUS[5] = CRC_Valid;
						STATUS[6] = 1;
						REQ_OUT = 1;
					end
					else if (Ack_internal_in)
						REQ_OUT = 0;
					CRC_Enable = 0;
					CRC_RST = 1;
					Cmd_Cnt = 0;
					cmd_oe_o = 0;
					CMD_OUT[39:0] = Out_Buff;
					Delay_Cnt = Delay_Cnt + 1;
				end
				ACK_WO: FSM_ACK = 1;
				ACK_WR: begin
					FSM_ACK = 1;
					REQ_OUT = 0;
				end
			endcase
	end
endmodule
module sd_crc_7 (
	BITVAL,
	Enable,
	CLK,
	RST,
	CRC
);
	input BITVAL;
	input Enable;
	input CLK;
	input RST;
	output reg [6:0] CRC;
	wire inv;
	assign inv = BITVAL ^ CRC[6];
	always @(posedge CLK or posedge RST)
		if (RST)
			CRC <= 0;
		else if (Enable == 1) begin
			CRC[6] <= CRC[5];
			CRC[5] <= CRC[4];
			CRC[4] <= CRC[3];
			CRC[3] <= CRC[2] ^ inv;
			CRC[2] <= CRC[1];
			CRC[1] <= CRC[0];
			CRC[0] <= inv;
		end
endmodule
