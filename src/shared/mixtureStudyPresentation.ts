import { localScheduleDateInput } from "./localScheduleForms";
export function mixtureStudyTime(value:string|null) { return value?localScheduleDateInput(value,"milliseconds").replace("T"," ")||"未知":"未知"; }
export function mixtureStudyReason(value:string) {
  const labels:Record<string,string>={insufficient_samples:"训练或留出样本不足",holdout_invalid:"留出样本不能用于验证",backtest_failed:"留出误差未通过，系数不发布",rank_deficient:"模型组合变化不足，无法区分单模型关系",ill_conditioned:"条件矩阵不稳定，暂不发布独立关系",invalid_number:"输入数值或计算结果无效",zero_weight_unpriced:"正 Token 命中了零系数，相关占用保持未知",history_incomplete:"历史覆盖不完整，相关窗口占用未知",source_binding_changed:"来源绑定已改变，需显式重新保存",source_unavailable:"来源当前不可用",privacy_mode:"隐私模式已隐藏手动实验",unknown_model_charge:"模型进入记录缺少可用的独立系数",fixed_mix_unpriced:"固定组合关系不能用于任意未来模型配比"};
  const native:Record<string,string>={fit_unavailable:"独立模型关系尚不可用，模型进入记录保持未知",prediction_out_of_range:"模型条件换算超出可用范围，相关占用保持未知",fixed_mixture_only:"只能识别所填固定组合，不能分离单模型权重"};
  return labels[value]??native[value]??value;
}
