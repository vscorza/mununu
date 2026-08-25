module present_encryptor_top (
	data_o,
	data_i,
	clk_i,
	key_load,
	data_load
);
	output wire [63:0] data_o;
	input [79:0] data_i;
	input clk_i;
	input key_load;
	input data_load;
	reg [63:0] state;
	reg [4:0] round_counter;
	reg [79:0] key;
	wire [63:0] round_key;
	wire [63:0] sub_per_input;
	wire [63:0] sub_per_output;
	wire [79:0] key_update_output;
	sub_per present_cipher_sp(
		.data_o(sub_per_output),
		.data_i(sub_per_input)
	);
	key_update present_cipher_key_update(
		.data_o(key_update_output),
		.data_i(key),
		.round_counter(round_counter)
	);
	assign round_key = key[79:16];
	assign sub_per_input = state ^ round_key;
	assign data_o = sub_per_input;
	always @(posedge clk_i)
		if (key_load)
			key <= data_i;
		else if (!key_load) begin
			if (data_load) begin
				state <= data_i[63:0];
				round_counter <= 5'b00001;
			end
			else if (!data_load) begin
				round_counter <= round_counter + 1'b1;
				state <= sub_per_output;
				key <= key_update_output;
			end
		end
endmodule
module key_update (
	data_o,
	data_i,
	round_counter
);
	output wire [79:0] data_o;
	input [79:0] data_i;
	input [4:0] round_counter;
	wire [79:0] s1;
	wire [79:0] s2;
	wire [79:0] s3;
	sbox key_update_sbox(
		.data_o(s2[79:76]),
		.data_i(s1[79:76])
	);
	assign s1 = {data_i[18:0], data_i[79:19]};
	assign s2[75:0] = s1[75:0];
	assign s3 = {s2[79:20], s2[19:15] ^ round_counter, s2[14:0]};
	assign data_o = s3;
endmodule
module permutation (
	data_o,
	data_i
);
	output wire [63:0] data_o;
	input [63:0] data_i;
	assign data_o[0] = data_i[0];
	assign data_o[16] = data_i[1];
	assign data_o[32] = data_i[2];
	assign data_o[48] = data_i[3];
	assign data_o[1] = data_i[4];
	assign data_o[17] = data_i[5];
	assign data_o[33] = data_i[6];
	assign data_o[49] = data_i[7];
	assign data_o[2] = data_i[8];
	assign data_o[18] = data_i[9];
	assign data_o[34] = data_i[10];
	assign data_o[50] = data_i[11];
	assign data_o[3] = data_i[12];
	assign data_o[19] = data_i[13];
	assign data_o[35] = data_i[14];
	assign data_o[51] = data_i[15];
	assign data_o[4] = data_i[16];
	assign data_o[20] = data_i[17];
	assign data_o[36] = data_i[18];
	assign data_o[52] = data_i[19];
	assign data_o[5] = data_i[20];
	assign data_o[21] = data_i[21];
	assign data_o[37] = data_i[22];
	assign data_o[53] = data_i[23];
	assign data_o[6] = data_i[24];
	assign data_o[22] = data_i[25];
	assign data_o[38] = data_i[26];
	assign data_o[54] = data_i[27];
	assign data_o[7] = data_i[28];
	assign data_o[23] = data_i[29];
	assign data_o[39] = data_i[30];
	assign data_o[55] = data_i[31];
	assign data_o[8] = data_i[32];
	assign data_o[24] = data_i[33];
	assign data_o[40] = data_i[34];
	assign data_o[56] = data_i[35];
	assign data_o[9] = data_i[36];
	assign data_o[25] = data_i[37];
	assign data_o[41] = data_i[38];
	assign data_o[57] = data_i[39];
	assign data_o[10] = data_i[40];
	assign data_o[26] = data_i[41];
	assign data_o[42] = data_i[42];
	assign data_o[58] = data_i[43];
	assign data_o[11] = data_i[44];
	assign data_o[27] = data_i[45];
	assign data_o[43] = data_i[46];
	assign data_o[59] = data_i[47];
	assign data_o[12] = data_i[48];
	assign data_o[28] = data_i[49];
	assign data_o[44] = data_i[50];
	assign data_o[60] = data_i[51];
	assign data_o[13] = data_i[52];
	assign data_o[29] = data_i[53];
	assign data_o[45] = data_i[54];
	assign data_o[61] = data_i[55];
	assign data_o[14] = data_i[56];
	assign data_o[30] = data_i[57];
	assign data_o[46] = data_i[58];
	assign data_o[62] = data_i[59];
	assign data_o[15] = data_i[60];
	assign data_o[31] = data_i[61];
	assign data_o[47] = data_i[62];
	assign data_o[63] = data_i[63];
endmodule
module sbox (
	data_o,
	data_i
);
	output reg [3:0] data_o;
	input [3:0] data_i;
	always @(data_i)
		case (data_i)
			4'h0: data_o = 4'hc;
			4'h1: data_o = 4'h5;
			4'h2: data_o = 4'h6;
			4'h3: data_o = 4'hb;
			4'h4: data_o = 4'h9;
			4'h5: data_o = 4'h0;
			4'h6: data_o = 4'ha;
			4'h7: data_o = 4'hd;
			4'h8: data_o = 4'h3;
			4'h9: data_o = 4'he;
			4'ha: data_o = 4'hf;
			4'hb: data_o = 4'h8;
			4'hc: data_o = 4'h4;
			4'hd: data_o = 4'h7;
			4'he: data_o = 4'h1;
			4'hf: data_o = 4'h2;
		endcase
endmodule
module sub_per (
	data_o,
	data_i
);
	output wire [63:0] data_o;
	input [63:0] data_i;
	wire [63:0] s;
	substitution sub_per_substitution(
		.data_o(s),
		.data_i(data_i)
	);
	permutation sub_per_permutation(
		.data_o(data_o),
		.data_i(s)
	);
endmodule
module substitution (
	data_o,
	data_i
);
	output wire [63:0] data_o;
	input [63:0] data_i;
	genvar _gv_j_1;
	generate
		for (_gv_j_1 = 0; _gv_j_1 < 16; _gv_j_1 = _gv_j_1 + 1) begin : boxes
			localparam j = _gv_j_1;
			sbox substitution_sbox(
				.data_o(data_o[(j * 4) + 3:j * 4]),
				.data_i(data_i[(j * 4) + 3:j * 4])
			);
		end
	endgenerate
endmodule
