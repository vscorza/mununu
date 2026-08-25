module t6507lp_fsm (
	clk,
	reset_n,
	alu_result,
	alu_status,
	data_in,
	alu_x,
	alu_y,
	address,
	rw_mem,
	data_out,
	alu_opcode,
	alu_a,
	alu_enable
);
	parameter [3:0] DATA_SIZE = 4'd8;
	parameter [3:0] ADDR_SIZE = 4'd13;
	localparam [3:0] DATA_SIZE_ = DATA_SIZE - 4'b0001;
	localparam [3:0] ADDR_SIZE_ = ADDR_SIZE - 4'b0001;
	input clk;
	input reset_n;
	input [DATA_SIZE_:0] alu_result;
	input [DATA_SIZE_:0] alu_status;
	input [DATA_SIZE_:0] data_in;
	input [DATA_SIZE_:0] alu_x;
	input [DATA_SIZE_:0] alu_y;
	output reg [ADDR_SIZE_:0] address;
	output reg rw_mem;
	output reg [DATA_SIZE_:0] data_out;
	output reg [DATA_SIZE_:0] alu_opcode;
	output reg [DATA_SIZE_:0] alu_a;
	output reg alu_enable;
	localparam FETCH_OP = 5'b00000;
	localparam FETCH_LOW = 5'b00010;
	localparam FETCH_HIGH = 5'b00011;
	localparam READ_MEM = 5'b00100;
	localparam DUMMY_WRT_CALC = 5'b00101;
	localparam WRITE_MEM = 5'b00110;
	localparam FETCH_OP_CALC_PARAM = 5'b00111;
	localparam READ_MEM_CALC_INDEX = 5'b01000;
	localparam FETCH_HIGH_CALC_INDEX = 5'b01001;
	localparam READ_MEM_FIX_ADDR = 5'b01010;
	localparam FETCH_OP_EVAL_BRANCH = 5'b01011;
	localparam FETCH_OP_FIX_PC = 5'b01100;
	localparam READ_FROM_POINTER = 5'b01101;
	localparam READ_FROM_POINTER_X = 5'b01110;
	localparam READ_FROM_POINTER_X1 = 5'b01111;
	localparam PUSH_PCH = 5'b10000;
	localparam PUSH_PCL = 5'b10001;
	localparam PUSH_STATUS = 5'b10010;
	localparam FETCH_PCL = 5'b10011;
	localparam FETCH_PCH = 5'b10100;
	localparam INCREMENT_SP = 5'b10101;
	localparam PULL_STATUS = 5'b10110;
	localparam PULL_PCL = 5'b10111;
	localparam PULL_PCH = 5'b11000;
	localparam INCREMENT_PC = 5'b11001;
	localparam PUSH_REGISTER = 5'b11010;
	localparam PULL_REGISTER = 5'b11011;
	localparam DUMMY = 5'b11100;
	localparam RESET = 5'b11111;
	localparam C = 3'b000;
	localparam Z = 3'b001;
	localparam I = 3'b010;
	localparam D = 3'b011;
	localparam B = 3'b100;
	localparam V = 3'b110;
	localparam N = 3'b111;
	localparam IMP = 4'h0;
	localparam ACC = 4'h1;
	localparam IMM = 4'h2;
	localparam ZPG = 4'h3;
	localparam ZPX = 4'h4;
	localparam ZPY = 4'h5;
	localparam REL = 4'h6;
	localparam ABS = 4'h7;
	localparam ABX = 4'h8;
	localparam ABY = 4'h9;
	localparam IDX = 4'ha;
	localparam IDY = 4'hb;
	localparam ADC_IMM = 8'h69;
	localparam ADC_ZPG = 8'h65;
	localparam ADC_ZPX = 8'h75;
	localparam ADC_ABS = 8'h6d;
	localparam ADC_ABX = 8'h7d;
	localparam ADC_ABY = 8'h79;
	localparam ADC_IDX = 8'h61;
	localparam ADC_IDY = 8'h71;
	localparam AND_IMM = 8'h29;
	localparam AND_ZPG = 8'h25;
	localparam AND_ZPX = 8'h35;
	localparam AND_ABS = 8'h2d;
	localparam AND_ABX = 8'h3d;
	localparam AND_ABY = 8'h39;
	localparam AND_IDX = 8'h21;
	localparam AND_IDY = 8'h31;
	localparam ASL_ACC = 8'h0a;
	localparam ASL_ZPG = 8'h06;
	localparam ASL_ZPX = 8'h16;
	localparam ASL_ABS = 8'h0e;
	localparam ASL_ABX = 8'h1e;
	localparam BCC_REL = 8'h90;
	localparam BCS_REL = 8'hb0;
	localparam BEQ_REL = 8'hf0;
	localparam BIT_ZPG = 8'h24;
	localparam BIT_ABS = 8'h2c;
	localparam BMI_REL = 8'h30;
	localparam BNE_REL = 8'hd0;
	localparam BPL_REL = 8'h10;
	localparam BRK_IMP = 8'h00;
	localparam BVC_REL = 8'h50;
	localparam BVS_REL = 8'h70;
	localparam CLC_IMP = 8'h18;
	localparam CLD_IMP = 8'hd8;
	localparam CLI_IMP = 8'h58;
	localparam CLV_IMP = 8'hb8;
	localparam CMP_IMM = 8'hc9;
	localparam CMP_ZPG = 8'hc5;
	localparam CMP_ZPX = 8'hd5;
	localparam CMP_ABS = 8'hcd;
	localparam CMP_ABX = 8'hdd;
	localparam CMP_ABY = 8'hd9;
	localparam CMP_IDX = 8'hc1;
	localparam CMP_IDY = 8'hd1;
	localparam CPX_IMM = 8'he0;
	localparam CPX_ZPG = 8'he4;
	localparam CPX_ABS = 8'hec;
	localparam CPY_IMM = 8'hc0;
	localparam CPY_ZPG = 8'hc4;
	localparam CPY_ABS = 8'hcc;
	localparam DEC_ZPG = 8'hc6;
	localparam DEC_ZPX = 8'hd6;
	localparam DEC_ABS = 8'hce;
	localparam DEC_ABX = 8'hde;
	localparam DEX_IMP = 8'hca;
	localparam DEY_IMP = 8'h88;
	localparam EOR_IMM = 8'h49;
	localparam EOR_ZPG = 8'h45;
	localparam EOR_ZPX = 8'h55;
	localparam EOR_ABS = 8'h4d;
	localparam EOR_ABX = 8'h5d;
	localparam EOR_ABY = 8'h59;
	localparam EOR_IDX = 8'h41;
	localparam EOR_IDY = 8'h51;
	localparam INC_ZPG = 8'he6;
	localparam INC_ZPX = 8'hf6;
	localparam INC_ABS = 8'hee;
	localparam INC_ABX = 8'hfe;
	localparam INX_IMP = 8'he8;
	localparam INY_IMP = 8'hc8;
	localparam JMP_ABS = 8'h4c;
	localparam JMP_IND = 8'h6c;
	localparam JSR_ABS = 8'h20;
	localparam LDA_IMM = 8'ha9;
	localparam LDA_ZPG = 8'ha5;
	localparam LDA_ZPX = 8'hb5;
	localparam LDA_ABS = 8'had;
	localparam LDA_ABX = 8'hbd;
	localparam LDA_ABY = 8'hb9;
	localparam LDA_IDX = 8'ha1;
	localparam LDA_IDY = 8'hb1;
	localparam LDX_IMM = 8'ha2;
	localparam LDX_ZPG = 8'ha6;
	localparam LDX_ZPY = 8'hb6;
	localparam LDX_ABS = 8'hae;
	localparam LDX_ABY = 8'hbe;
	localparam LDY_IMM = 8'ha0;
	localparam LDY_ZPG = 8'ha4;
	localparam LDY_ZPX = 8'hb4;
	localparam LDY_ABS = 8'hac;
	localparam LDY_ABX = 8'hbc;
	localparam LSR_ACC = 8'h4a;
	localparam LSR_ZPG = 8'h46;
	localparam LSR_ZPX = 8'h56;
	localparam LSR_ABS = 8'h4e;
	localparam LSR_ABX = 8'h5e;
	localparam NOP_IMP = 8'hea;
	localparam ORA_IMM = 8'h09;
	localparam ORA_ZPG = 8'h05;
	localparam ORA_ZPX = 8'h15;
	localparam ORA_ABS = 8'h0d;
	localparam ORA_ABX = 8'h1d;
	localparam ORA_ABY = 8'h19;
	localparam ORA_IDX = 8'h01;
	localparam ORA_IDY = 8'h11;
	localparam PHA_IMP = 8'h48;
	localparam PHP_IMP = 8'h08;
	localparam PLA_IMP = 8'h68;
	localparam PLP_IMP = 8'h28;
	localparam ROL_ACC = 8'h2a;
	localparam ROL_ZPG = 8'h26;
	localparam ROL_ZPX = 8'h36;
	localparam ROL_ABS = 8'h2e;
	localparam ROL_ABX = 8'h3e;
	localparam ROR_ACC = 8'h6a;
	localparam ROR_ZPG = 8'h66;
	localparam ROR_ZPX = 8'h76;
	localparam ROR_ABS = 8'h6e;
	localparam ROR_ABX = 8'h7e;
	localparam RTI_IMP = 8'h40;
	localparam RTS_IMP = 8'h60;
	localparam SBC_IMM = 8'he9;
	localparam SBC_ZPG = 8'he5;
	localparam SBC_ZPX = 8'hf5;
	localparam SBC_ABS = 8'hed;
	localparam SBC_ABX = 8'hfd;
	localparam SBC_ABY = 8'hf9;
	localparam SBC_IDX = 8'he1;
	localparam SBC_IDY = 8'hf1;
	localparam SEC_IMP = 8'h38;
	localparam SED_IMP = 8'hf8;
	localparam SEI_IMP = 8'h78;
	localparam STA_ZPG = 8'h85;
	localparam STA_ZPX = 8'h95;
	localparam STA_ABS = 8'h8d;
	localparam STA_ABX = 8'h9d;
	localparam STA_ABY = 8'h99;
	localparam STA_IDX = 8'h81;
	localparam STA_IDY = 8'h91;
	localparam STX_ZPG = 8'h86;
	localparam STX_ZPY = 8'h96;
	localparam STX_ABS = 8'h8e;
	localparam STY_ZPG = 8'h84;
	localparam STY_ZPX = 8'h94;
	localparam STY_ABS = 8'h8c;
	localparam TAX_IMP = 8'haa;
	localparam TAY_IMP = 8'ha8;
	localparam TSX_IMP = 8'hba;
	localparam TXA_IMP = 8'h8a;
	localparam TXS_IMP = 8'h9a;
	localparam TYA_IMP = 8'h98;
	localparam MEM_READ = 1'b0;
	localparam MEM_WRITE = 1'b1;
	reg [ADDR_SIZE_:0] pc;
	reg [DATA_SIZE:0] sp;
	reg [DATA_SIZE_:0] ir;
	reg [ADDR_SIZE_:0] temp_addr;
	reg [DATA_SIZE_:0] temp_data;
	reg [4:0] state;
	reg [4:0] next_state;
	reg absolute;
	reg absolute_indexed;
	reg accumulator;
	reg immediate;
	reg implied;
	reg indirectx;
	reg indirecty;
	reg relative;
	reg zero_page;
	reg zero_page_indexed;
	reg [DATA_SIZE_:0] index;
	reg read;
	reg read_modify_write;
	reg write;
	reg jump;
	reg jump_indirect;
	reg index_is_x;
	reg index_is_branch;
	reg brk;
	reg rti;
	reg rts;
	reg pha;
	reg php;
	reg pla;
	reg plp;
	reg jsr;
	reg tsx;
	reg txs;
	reg nop;
	reg invalid;
	wire [ADDR_SIZE_:0] next_pc;
	assign next_pc = pc + 13'b0000000000001;
	wire [DATA_SIZE:0] sp_plus_one;
	assign sp_plus_one = {1'b1, sp[7:0] + 8'b00000001};
	wire [DATA_SIZE:0] sp_minus_one;
	assign sp_minus_one = {1'b1, sp[7:0] - 8'b00000001};
	reg [ADDR_SIZE_:0] address_plus_index;
	reg page_crossed;
	reg branch;
	always @(*) begin
		address_plus_index = 13'h0000;
		page_crossed = 1'b0;
		case (state)
			READ_MEM_FIX_ADDR, FETCH_HIGH_CALC_INDEX: begin
				{page_crossed, address_plus_index[7:0]} = temp_addr[7:0] + index;
				address_plus_index[12:8] = temp_addr[12:8] + page_crossed;
			end
			READ_FROM_POINTER_X1: begin
				{page_crossed, address_plus_index[7:0]} = temp_addr[7:0] + index;
				address_plus_index[12:8] = data_in[4:0];
			end
			FETCH_OP_FIX_PC, FETCH_OP_EVAL_BRANCH:
				if (branch) begin
					{page_crossed, address_plus_index[7:0]} = pc[7:0] + index;
					address_plus_index[12:8] = pc[12:8] + page_crossed;
				end
			READ_FROM_POINTER:
				if (indirectx)
					{page_crossed, address_plus_index[7:0]} = temp_data + index;
				else if (jump_indirect)
					address_plus_index[7:0] = temp_addr[7:0] + 8'h01;
				else
					address_plus_index[7:0] = temp_data + 8'h01;
			READ_FROM_POINTER_X: {page_crossed, address_plus_index[7:0]} = (temp_data + index) + 8'h01;
			READ_MEM_CALC_INDEX: {page_crossed, address_plus_index[7:0]} = temp_addr[7:0] + index;
		endcase
	end
	reg [2:0] rst_counter;
	always @(posedge clk or negedge reset_n)
		if (reset_n == 1'b0) begin
			pc <= 13'h0000;
			sp <= 9'b111111111;
			ir <= 8'h00;
			temp_addr <= 13'h0000;
			temp_data <= 8'h00;
			state <= RESET;
			address <= 13'h0000;
			rw_mem <= MEM_READ;
			data_out <= 8'h00;
			rst_counter <= 3'h0;
			index <= 8'h00;
		end
		else begin
			state <= next_state;
			case (state)
				RESET: rst_counter <= rst_counter + 3'b001;
				FETCH_OP, FETCH_OP_CALC_PARAM: begin
					pc <= next_pc;
					address <= next_pc;
					rw_mem <= MEM_READ;
					ir <= data_in;
				end
				FETCH_LOW: begin
					if (index_is_x == 1'b1)
						index <= alu_x;
					else
						index <= alu_y;
					if (index_is_branch)
						index <= temp_data;
					if (((accumulator || implied) || txs) || tsx) begin
						pc <= pc;
						address <= pc;
						rw_mem <= MEM_READ;
						if (txs)
							sp[7:0] <= alu_x;
					end
					else if (immediate || relative) begin
						pc <= next_pc;
						address <= next_pc;
						rw_mem <= MEM_READ;
						temp_data <= data_in;
					end
					else if ((absolute || absolute_indexed) || jump_indirect) begin
						pc <= next_pc;
						address <= next_pc;
						rw_mem <= MEM_READ;
						temp_addr <= {{5 {1'b0}}, data_in};
						temp_data <= 8'h00;
					end
					else if (zero_page) begin
						pc <= next_pc;
						address <= {{5 {1'b0}}, data_in};
						temp_addr <= {{5 {1'b0}}, data_in};
						if (write) begin
							rw_mem <= MEM_WRITE;
							data_out <= alu_result;
						end
						else begin
							rw_mem <= MEM_READ;
							data_out <= 8'h00;
						end
					end
					else if (zero_page_indexed) begin
						pc <= next_pc;
						address <= {{5 {1'b0}}, data_in};
						temp_addr <= {{5 {1'b0}}, data_in};
						rw_mem <= MEM_READ;
					end
					else if (indirectx || indirecty) begin
						pc <= next_pc;
						address <= data_in;
						temp_data <= data_in;
						rw_mem <= MEM_READ;
					end
					else if (brk) begin
						pc <= next_pc;
						address <= sp;
						data_out <= {{3 {1'b0}}, pc[12:8]};
						rw_mem <= MEM_WRITE;
					end
					else if (rti || rts) begin
						address <= sp;
						rw_mem <= MEM_READ;
					end
					else if (pha || php) begin
						pc <= pc;
						address <= sp;
						data_out <= (pha ? alu_result : alu_status);
						rw_mem <= MEM_WRITE;
					end
					else if (pla || plp) begin
						pc <= pc;
						address <= sp;
						rw_mem <= MEM_READ;
					end
					else if (invalid) begin
						address <= pc;
						rw_mem <= MEM_READ;
					end
					else begin
						address <= sp;
						rw_mem <= MEM_READ;
						temp_addr <= {{5 {1'b0}}, data_in};
						pc <= next_pc;
					end
				end
				FETCH_HIGH_CALC_INDEX: begin
					pc <= next_pc;
					temp_addr[12:8] <= data_in[4:0];
					address <= {data_in[4:0], address_plus_index[7:0]};
					rw_mem <= MEM_READ;
					data_out <= 8'h00;
				end
				FETCH_OP_EVAL_BRANCH:
					if (branch) begin
						pc <= {{5 {1'b0}}, address_plus_index[7:0]};
						address <= {{5 {1'b0}}, address_plus_index[7:0]};
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
					end
					else begin
						pc <= next_pc;
						address <= next_pc;
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
						ir <= data_in;
					end
				FETCH_OP_FIX_PC:
					if (page_crossed) begin
						pc[12:8] <= address_plus_index[12:8];
						address[12:8] <= address_plus_index[12:8];
					end
					else begin
						pc <= next_pc;
						address <= next_pc;
						rw_mem <= MEM_READ;
						ir <= data_in;
					end
				FETCH_HIGH:
					if (jump) begin
						pc <= {data_in[4:0], temp_addr[7:0]};
						address <= {data_in[4:0], temp_addr[7:0]};
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
					end
					else if (write) begin
						pc <= next_pc;
						temp_addr[12:8] <= data_in[4:0];
						address <= {data_in[4:0], temp_addr[7:0]};
						rw_mem <= MEM_WRITE;
						data_out <= alu_result;
					end
					else begin
						pc <= next_pc;
						temp_addr[12:8] <= data_in[4:0];
						address <= {data_in[4:0], temp_addr[7:0]};
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
					end
				READ_MEM:
					if (read_modify_write) begin
						pc <= pc;
						address <= temp_addr;
						rw_mem <= MEM_WRITE;
						temp_data <= data_in;
						data_out <= data_in;
					end
					else begin
						pc <= pc;
						address <= pc;
						temp_data <= data_in;
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
					end
				READ_MEM_CALC_INDEX: begin
					address <= address_plus_index;
					temp_addr <= address_plus_index;
					if (write) begin
						rw_mem <= MEM_WRITE;
						data_out <= alu_result;
					end
					else begin
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
					end
				end
				READ_MEM_FIX_ADDR:
					if (read) begin
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
						if (page_crossed) begin
							address <= address_plus_index;
							temp_addr <= address_plus_index;
						end
						else begin
							address <= pc;
							temp_data <= data_in;
						end
					end
					else if (write) begin
						rw_mem <= MEM_WRITE;
						data_out <= alu_result;
						address <= address_plus_index;
						temp_addr <= address_plus_index;
					end
					else begin
						rw_mem <= MEM_READ;
						data_out <= 8'h00;
						address <= address_plus_index;
						temp_addr <= address_plus_index;
					end
				DUMMY_WRT_CALC: begin
					pc <= pc;
					address <= temp_addr;
					rw_mem <= MEM_WRITE;
					data_out <= alu_result;
				end
				WRITE_MEM: begin
					pc <= pc;
					address <= pc;
					rw_mem <= MEM_READ;
					data_out <= 8'h00;
				end
				READ_FROM_POINTER:
					if (jump_indirect) begin
						pc[7:0] <= data_in;
						rw_mem <= MEM_READ;
						address <= address_plus_index;
					end
					else begin
						pc <= pc;
						rw_mem <= MEM_READ;
						if (indirectx)
							address <= address_plus_index;
						else begin
							address <= address_plus_index;
							temp_addr <= {{5 {1'b0}}, data_in};
						end
					end
				READ_FROM_POINTER_X: begin
					pc <= pc;
					address <= address_plus_index;
					temp_addr[7:0] <= data_in;
					rw_mem <= MEM_READ;
				end
				READ_FROM_POINTER_X1:
					if (jump_indirect) begin
						pc[12:8] <= data_in[4:0];
						rw_mem <= MEM_READ;
						address <= {data_in[4:0], pc[7:0]};
					end
					else if (indirectx) begin
						address <= {data_in[4:0], temp_addr[7:0]};
						if (write) begin
							rw_mem <= MEM_WRITE;
							data_out <= alu_result;
						end
						else
							rw_mem <= MEM_READ;
					end
					else begin
						address <= address_plus_index;
						temp_addr[12:8] <= data_in;
						rw_mem <= MEM_READ;
					end
				PUSH_PCH: begin
					pc <= pc;
					address <= sp_minus_one;
					data_out <= pc[7:0];
					rw_mem <= MEM_WRITE;
					sp <= sp_minus_one;
				end
				PUSH_PCL:
					if (jsr) begin
						pc <= pc;
						address <= pc;
						rw_mem <= MEM_READ;
						sp <= sp_minus_one;
					end
					else begin
						pc <= pc;
						address <= sp_minus_one;
						data_out <= alu_status;
						rw_mem <= MEM_WRITE;
						sp <= sp_minus_one;
					end
				PUSH_STATUS: begin
					address <= 13'h1ffe;
					rw_mem <= MEM_READ;
					sp <= sp_minus_one;
				end
				FETCH_PCL: begin
					pc[7:0] <= data_in;
					address <= 13'h1fff;
					rw_mem <= MEM_READ;
				end
				FETCH_PCH: begin
					pc[12:8] <= data_in[4:0];
					address <= {data_in[4:0], pc[7:0]};
					rw_mem <= MEM_READ;
				end
				INCREMENT_SP: begin
					sp <= sp_plus_one;
					address <= sp_plus_one;
				end
				PULL_STATUS: begin
					sp <= sp_plus_one;
					address <= sp_plus_one;
					temp_data <= data_in;
				end
				PULL_PCL: begin
					sp <= sp_plus_one;
					address <= sp_plus_one;
					pc[7:0] <= data_in;
				end
				PULL_PCH: begin
					pc[12:8] <= data_in[4:0];
					address <= {data_in[4:0], pc[7:0]};
				end
				INCREMENT_PC: begin
					pc <= next_pc;
					address <= next_pc;
				end
				PUSH_REGISTER: begin
					pc <= pc;
					address <= pc;
					sp <= sp_minus_one;
					rw_mem <= MEM_READ;
					temp_data <= data_in;
				end
				PULL_REGISTER: begin
					pc <= pc;
					address <= pc;
					temp_data <= data_in;
				end
				DUMMY: begin
					address <= sp;
					rw_mem <= MEM_WRITE;
				end
				default:
					;
			endcase
		end
	always @(*) begin
		alu_opcode = 8'h00;
		alu_a = 8'h00;
		alu_enable = 1'b0;
		next_state = RESET;
		if (invalid == 1'b1)
			next_state = FETCH_OP;
		else
			case (state)
				RESET:
					if (rst_counter == 3'd6)
						next_state = FETCH_OP;
				FETCH_OP: next_state = FETCH_LOW;
				FETCH_OP_CALC_PARAM: begin
					next_state = FETCH_LOW;
					alu_opcode = ir;
					alu_enable = 1'b1;
					alu_a = temp_data;
				end
				FETCH_LOW:
					if ((accumulator || implied) || txs) begin
						if (!nop) begin
							alu_opcode = ir;
							alu_enable = 1'b1;
						end
						next_state = FETCH_OP;
					end
					else if (tsx) begin
						alu_opcode = ir;
						alu_enable = 1'b1;
						next_state = FETCH_OP;
						alu_a = sp[7:0];
					end
					else if (immediate)
						next_state = FETCH_OP_CALC_PARAM;
					else if (zero_page) begin
						if (read || read_modify_write)
							next_state = READ_MEM;
						else if (write) begin
							next_state = WRITE_MEM;
							alu_opcode = ir;
							alu_enable = 1'b1;
							alu_a = 8'h00;
						end
					end
					else if (zero_page_indexed)
						next_state = READ_MEM_CALC_INDEX;
					else if (absolute || jump_indirect) begin
						next_state = FETCH_HIGH;
						if (write) begin
							alu_opcode = ir;
							alu_enable = 1'b1;
							alu_a = 8'h00;
						end
					end
					else if (absolute_indexed)
						next_state = FETCH_HIGH_CALC_INDEX;
					else if (relative)
						next_state = FETCH_OP_EVAL_BRANCH;
					else if (indirectx || indirecty)
						next_state = READ_FROM_POINTER;
					else if (brk)
						next_state = PUSH_PCH;
					else if (rti || rts)
						next_state = INCREMENT_SP;
					else if (pha) begin
						alu_opcode = ir;
						alu_enable = 1'b1;
						next_state = PUSH_REGISTER;
					end
					else if (php)
						next_state = PUSH_REGISTER;
					else if (pla || plp)
						next_state = INCREMENT_SP;
					else
						next_state = DUMMY;
				READ_FROM_POINTER:
					if (indirectx)
						next_state = READ_FROM_POINTER_X;
					else
						next_state = READ_FROM_POINTER_X1;
				READ_FROM_POINTER_X: next_state = READ_FROM_POINTER_X1;
				READ_FROM_POINTER_X1:
					if (jump_indirect)
						next_state = FETCH_OP;
					else if (indirecty)
						next_state = READ_MEM_FIX_ADDR;
					else if (read)
						next_state = READ_MEM;
					else if (write) begin
						alu_opcode = ir;
						alu_enable = 1'b1;
						next_state = WRITE_MEM;
					end
				FETCH_OP_EVAL_BRANCH:
					if (branch)
						next_state = FETCH_OP_FIX_PC;
					else
						next_state = FETCH_LOW;
				FETCH_OP_FIX_PC:
					if (page_crossed)
						next_state = FETCH_OP;
					else
						next_state = FETCH_LOW;
				FETCH_HIGH_CALC_INDEX: next_state = READ_MEM_FIX_ADDR;
				READ_MEM_FIX_ADDR:
					if (read) begin
						if (page_crossed)
							next_state = READ_MEM;
						else
							next_state = FETCH_OP_CALC_PARAM;
					end
					else if (read_modify_write)
						next_state = READ_MEM;
					else if (write) begin
						next_state = WRITE_MEM;
						alu_enable = 1'b1;
						alu_opcode = ir;
					end
				FETCH_HIGH:
					if (jump_indirect)
						next_state = READ_FROM_POINTER;
					else if (jump)
						next_state = FETCH_OP;
					else if (read || read_modify_write)
						next_state = READ_MEM;
					else if (write)
						next_state = WRITE_MEM;
				READ_MEM_CALC_INDEX:
					if (read || read_modify_write)
						next_state = READ_MEM;
					else if (write) begin
						alu_opcode = ir;
						alu_enable = 1'b1;
						next_state = WRITE_MEM;
					end
				READ_MEM:
					if (read)
						next_state = FETCH_OP_CALC_PARAM;
					else if (read_modify_write)
						next_state = DUMMY_WRT_CALC;
				DUMMY_WRT_CALC: begin
					alu_opcode = ir;
					alu_enable = 1'b1;
					alu_a = data_in;
					next_state = WRITE_MEM;
				end
				WRITE_MEM: next_state = FETCH_OP;
				PUSH_PCH: next_state = PUSH_PCL;
				PUSH_PCL:
					if (jsr)
						next_state = FETCH_HIGH;
					else
						next_state = PUSH_STATUS;
				PUSH_STATUS: next_state = FETCH_PCL;
				FETCH_PCL: next_state = FETCH_PCH;
				FETCH_PCH: next_state = FETCH_OP;
				INCREMENT_SP:
					if (rti)
						next_state = PULL_STATUS;
					else if (pla || plp)
						next_state = PULL_REGISTER;
					else
						next_state = PULL_PCL;
				PULL_STATUS: next_state = PULL_PCL;
				PULL_PCL: begin
					next_state = PULL_PCH;
					if (rti) begin
						alu_opcode = ir;
						alu_enable = 1'b1;
						alu_a = temp_data;
					end
				end
				PULL_PCH:
					if (rti)
						next_state = FETCH_OP;
					else
						next_state = INCREMENT_PC;
				INCREMENT_PC: next_state = FETCH_OP;
				PUSH_REGISTER: next_state = FETCH_OP;
				PULL_REGISTER: next_state = FETCH_OP_CALC_PARAM;
				DUMMY: next_state = PUSH_PCH;
				default: next_state = RESET;
			endcase
	end
	always @(*) begin
		absolute = 1'b0;
		absolute_indexed = 1'b0;
		accumulator = 1'b0;
		immediate = 1'b0;
		implied = 1'b0;
		indirectx = 1'b0;
		indirecty = 1'b0;
		relative = 1'b0;
		zero_page = 1'b0;
		zero_page_indexed = 1'b0;
		index_is_x = 1'b0;
		index_is_branch = 1'b0;
		read = 1'b0;
		read_modify_write = 1'b0;
		write = 1'b0;
		jump = 1'b0;
		jump_indirect = 1'b0;
		branch = 1'b0;
		brk = 1'b0;
		rti = 1'b0;
		rts = 1'b0;
		pha = 1'b0;
		php = 1'b0;
		pla = 1'b0;
		plp = 1'b0;
		jsr = 1'b0;
		tsx = 1'b0;
		txs = 1'b0;
		nop = 1'b0;
		invalid = 1'b0;
		case (ir)
			CLC_IMP, CLD_IMP, CLI_IMP, CLV_IMP, DEX_IMP, DEY_IMP, INX_IMP, INY_IMP, SEC_IMP, SED_IMP, SEI_IMP, TAX_IMP, TAY_IMP, TXA_IMP, TYA_IMP: implied = 1'b1;
			NOP_IMP: begin
				implied = 1'b1;
				nop = 1'b1;
			end
			ASL_ACC, LSR_ACC, ROL_ACC, ROR_ACC: accumulator = 1'b1;
			ADC_IMM, AND_IMM, CMP_IMM, CPX_IMM, CPY_IMM, EOR_IMM, LDA_IMM, LDX_IMM, LDY_IMM, ORA_IMM, SBC_IMM: immediate = 1'b1;
			ADC_ZPG, AND_ZPG, ASL_ZPG, BIT_ZPG, CMP_ZPG, CPX_ZPG, CPY_ZPG, DEC_ZPG, EOR_ZPG, INC_ZPG, LDA_ZPG, LDX_ZPG, LDY_ZPG, LSR_ZPG, ORA_ZPG, ROL_ZPG, ROR_ZPG, SBC_ZPG, STA_ZPG, STX_ZPG, STY_ZPG: zero_page = 1'b1;
			ADC_ZPX, AND_ZPX, ASL_ZPX, CMP_ZPX, DEC_ZPX, EOR_ZPX, INC_ZPX, LDA_ZPX, LDY_ZPX, LSR_ZPX, ORA_ZPX, ROL_ZPX, ROR_ZPX, SBC_ZPX, STA_ZPX, STY_ZPX: begin
				zero_page_indexed = 1'b1;
				index_is_x = 1'b1;
			end
			LDX_ZPY, STX_ZPY: begin
				zero_page_indexed = 1'b1;
				index_is_x = 1'b0;
			end
			BCC_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (!alu_status[C])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BCS_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (alu_status[C])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BEQ_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (alu_status[Z])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BNE_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (alu_status[Z] == 1'b0)
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BPL_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (!alu_status[N])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BMI_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (alu_status[N])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BVC_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (!alu_status[V])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			BVS_REL: begin
				relative = 1'b1;
				index_is_branch = 1'b1;
				if (alu_status[V])
					branch = 1'b1;
				else
					branch = 1'b0;
			end
			ADC_ABS, AND_ABS, ASL_ABS, BIT_ABS, CMP_ABS, CPX_ABS, CPY_ABS, DEC_ABS, EOR_ABS, INC_ABS, LDA_ABS, LDX_ABS, LDY_ABS, LSR_ABS, ORA_ABS, ROL_ABS, ROR_ABS, SBC_ABS, STA_ABS, STX_ABS, STY_ABS: absolute = 1'b1;
			ADC_ABX, AND_ABX, ASL_ABX, CMP_ABX, DEC_ABX, EOR_ABX, INC_ABX, LDA_ABX, LDY_ABX, LSR_ABX, ORA_ABX, ROL_ABX, ROR_ABX, SBC_ABX, STA_ABX: begin
				absolute_indexed = 1'b1;
				index_is_x = 1'b1;
			end
			ADC_ABY, AND_ABY, CMP_ABY, EOR_ABY, LDA_ABY, LDX_ABY, ORA_ABY, SBC_ABY, STA_ABY: begin
				absolute_indexed = 1'b1;
				index_is_x = 1'b0;
			end
			ADC_IDX, AND_IDX, CMP_IDX, EOR_IDX, LDA_IDX, ORA_IDX, SBC_IDX, STA_IDX: begin
				indirectx = 1'b1;
				index_is_x = 1'b1;
			end
			ADC_IDY, AND_IDY, CMP_IDY, EOR_IDY, LDA_IDY, ORA_IDY, SBC_IDY, STA_IDY: begin
				indirecty = 1'b1;
				index_is_x = 1'b0;
			end
			JMP_ABS: begin
				absolute = 1'b1;
				jump = 1'b1;
			end
			JMP_IND: jump_indirect = 1'b1;
			BRK_IMP: brk = 1'b1;
			RTI_IMP: rti = 1'b1;
			RTS_IMP: rts = 1'b1;
			PHA_IMP: pha = 1'b1;
			PHP_IMP: php = 1'b1;
			PLA_IMP: pla = 1'b1;
			PLP_IMP: plp = 1'b1;
			JSR_ABS: begin
				jsr = 1'b1;
				jump = 1'b1;
			end
			TSX_IMP: tsx = 1'b1;
			TXS_IMP: txs = 1'b1;
			default: begin
				index_is_x = 1'b1;
				if ((reset_n == 1'b1) && (state != FETCH_OP_FIX_PC))
					invalid = 1'b1;
			end
		endcase
		case (ir)
			ASL_ACC, ASL_ZPG, ASL_ZPX, ASL_ABS, ASL_ABX, LSR_ACC, LSR_ZPG, LSR_ZPX, LSR_ABS, LSR_ABX, ROL_ACC, ROL_ZPG, ROL_ZPX, ROL_ABS, ROL_ABX, ROR_ACC, ROR_ZPG, ROR_ZPX, ROR_ABS, ROR_ABX, INC_ZPG, INC_ZPX, INC_ABS, INC_ABX, DEC_ZPG, DEC_ZPX, DEC_ABS, DEC_ABX: read_modify_write = 1'b1;
			STA_ZPG, STA_ZPX, STA_ABS, STA_ABX, STA_ABY, STA_IDX, STA_IDY, STX_ZPG, STX_ZPY, STX_ABS, STY_ZPG, STY_ZPX, STY_ABS: write = 1'b1;
			default: read = 1'b1;
		endcase
	end
endmodule
