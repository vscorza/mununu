// @mununu_guarantee nu Z. (((start_tx_i != 1 and start_rx_i != 1) or (mu X. (trans_done == 1 or [] X))) and [] Z)
module sd_data_master (
	sd_clk,
	rst,
	start_tx_i,
	start_rx_i,
	d_write_o,
	d_read_o,
	start_tx_fifo_o,
	start_rx_fifo_o,
	tx_fifo_empty_i,
	tx_fifo_full_i,
	rx_fifo_full_i,
	xfr_complete_i,
	crc_ok_i,
	int_status_o,
	int_status_rst_i
);
	input sd_clk;
	input rst;
	input start_tx_i;
	input start_rx_i;
	output reg d_write_o;
	output reg d_read_o;
	output reg start_tx_fifo_o;
	output reg start_rx_fifo_o;
	input tx_fifo_empty_i;
	input tx_fifo_full_i;
	input rx_fifo_full_i;
	input xfr_complete_i;
	input crc_ok_i;
	output reg [2:0] int_status_o;
	input int_status_rst_i;
	reg tx_cycle;
	parameter SIZE = 3;
	reg [SIZE - 1:0] state;
	reg [SIZE - 1:0] next_state;
	parameter IDLE = 3'b000;
	parameter START_TX_FIFO = 3'b001;
	parameter START_RX_FIFO = 3'b010;
	parameter DATA_TRANSFER = 3'b100;
	reg trans_done;
	always @(state or start_tx_i or start_rx_i or tx_fifo_full_i or xfr_complete_i or trans_done) begin : FSM_COMBO
		case (state)
			IDLE:
				if (start_tx_i == 1)
					next_state <= START_TX_FIFO;
				else if (start_rx_i == 1)
					next_state <= START_RX_FIFO;
				else
					next_state <= IDLE;
			START_TX_FIFO:
				if ((tx_fifo_full_i == 1) && (xfr_complete_i == 0))
					next_state <= DATA_TRANSFER;
				else
					next_state <= START_TX_FIFO;
			START_RX_FIFO:
				if (xfr_complete_i == 0)
					next_state <= DATA_TRANSFER;
				else
					next_state <= START_RX_FIFO;
			DATA_TRANSFER:
				if (trans_done)
					next_state <= IDLE;
				else
					next_state <= DATA_TRANSFER;
			default: next_state <= IDLE;
		endcase
	end
	always @(posedge sd_clk or posedge rst) begin : FSM_SEQ
		if (rst)
			state <= IDLE;
		else
			state <= next_state;
	end
	always @(posedge sd_clk or posedge rst)
		if (rst) begin
			start_tx_fifo_o <= 0;
			start_rx_fifo_o <= 0;
			d_write_o <= 0;
			d_read_o <= 0;
			trans_done <= 0;
			tx_cycle <= 0;
			int_status_o <= 0;
		end
		else begin
			case (state)
				IDLE: begin
					start_tx_fifo_o <= 0;
					start_rx_fifo_o <= 0;
					d_write_o <= 0;
					d_read_o <= 0;
					trans_done <= 0;
					tx_cycle <= 0;
				end
				START_RX_FIFO: begin
					start_rx_fifo_o <= 1;
					start_tx_fifo_o <= 0;
					tx_cycle <= 0;
					d_read_o <= 1;
				end
				START_TX_FIFO: begin
					start_rx_fifo_o <= 0;
					start_tx_fifo_o <= 1;
					tx_cycle <= 1;
					if (tx_fifo_full_i == 1)
						d_write_o <= 1;
				end
				DATA_TRANSFER: begin
					d_read_o <= 0;
					d_write_o <= 0;
					if (tx_cycle) begin
						if (tx_fifo_empty_i) begin
							if (!trans_done)
								int_status_o[2] <= 1;
							trans_done <= 1;
							d_write_o <= 1;
							d_read_o <= 1;
						end
					end
					else if (rx_fifo_full_i) begin
						if (!trans_done)
							int_status_o[2] <= 1;
						trans_done <= 1;
						d_write_o <= 1;
						d_read_o <= 1;
					end
					if (xfr_complete_i) begin
						d_write_o <= 0;
						d_read_o <= 0;
						trans_done <= 1;
						if (!crc_ok_i) begin
							if (!trans_done)
								int_status_o[1] <= 1;
						end
						else if (crc_ok_i) begin
							if (!trans_done)
								int_status_o[0] <= 1;
						end
					end
				end
			endcase
			if (int_status_rst_i)
				int_status_o <= 0;
		end
endmodule
