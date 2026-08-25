(* blackbox *)
module i2c_master_bit_ctrl(clk,rst,nReset,ena,clk_cnt,cmd,cmd_ack,busy,al,din,dout,scl_i,scl_o,scl_oen,sda_i,sda_o,sda_oen);
  input clk,rst,nReset,ena; input [15:0] clk_cnt; input [3:0] cmd;
  output cmd_ack,busy,al; input din; output dout;
  input scl_i; output scl_o,scl_oen; input sda_i; output sda_o,sda_oen;
endmodule
