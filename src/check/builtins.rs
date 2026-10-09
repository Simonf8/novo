use super::types::Ty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Len,
    Range,
    ToStr,
    ToInt,
    ToFloat,
    ToBool,
    Abs,
    Min,
    Max,
    Round,
    Assert,
    AssertEq,
    Fail,
    TypeOf,
    Input,
    MathPi,
    MathE,
    MathSqrt,
    MathPow,
    MathFloor,
    MathCeil,
    MathSin,
    MathCos,
    MathTan,
    MathLog,
    MathExp,
    TimeNow,
    TimeNowMs,
    TimeSleep,
    JsonParse,
    JsonStringify,
    FsRead,
    FsWrite,
    FsAppend,
    FsExists,
    FsListDir,
    FsDelete,
    FsCreateDir,
    EnvGet,
    EnvSet,
    EnvArgs,
    NnTensor,
    NnZeros,
    NnOnes,
    NnRandn,
    NnRand,
    NnArange,
    NnEye,
    NnParam,
    NnSeed,
    NnSgd,
    NnAdam,
    NnMse,
    NnCrossEntropy,
    NnStack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdModule {
    Math,
    Time,
    Json,
    Fs,
    Env,
    Nn,
}

impl StdModule {
    pub fn from_name(name: &str) -> Option<StdModule> {
        Some(match name {
            "math" => StdModule::Math,
            "time" => StdModule::Time,
            "json" => StdModule::Json,
            "fs" => StdModule::Fs,
            "env" => StdModule::Env,
            "nn" => StdModule::Nn,
            _ => return None,
        })
    }

    pub fn member(self, name: &str) -> Option<Builtin> {
        use Builtin::*;
        Some(match (self, name) {
            (StdModule::Math, "pi") => MathPi,
            (StdModule::Math, "e") => MathE,
            (StdModule::Math, "sqrt") => MathSqrt,
            (StdModule::Math, "pow") => MathPow,
            (StdModule::Math, "abs") => Abs,
            (StdModule::Math, "floor") => MathFloor,
            (StdModule::Math, "ceil") => MathCeil,
            (StdModule::Math, "round") => Round,
            (StdModule::Math, "sin") => MathSin,
            (StdModule::Math, "cos") => MathCos,
            (StdModule::Math, "tan") => MathTan,
            (StdModule::Math, "log") => MathLog,
            (StdModule::Math, "exp") => MathExp,
            (StdModule::Math, "min") => Min,
            (StdModule::Math, "max") => Max,
            (StdModule::Time, "now") => TimeNow,
            (StdModule::Time, "now_ms") => TimeNowMs,
            (StdModule::Time, "sleep") => TimeSleep,
            (StdModule::Json, "parse") => JsonParse,
            (StdModule::Json, "stringify") => JsonStringify,
            (StdModule::Fs, "read_file") => FsRead,
            (StdModule::Fs, "write_file") => FsWrite,
            (StdModule::Fs, "append_file") => FsAppend,
            (StdModule::Fs, "exists") => FsExists,
            (StdModule::Fs, "list_dir") => FsListDir,
            (StdModule::Fs, "delete_file") => FsDelete,
            (StdModule::Fs, "create_dir") => FsCreateDir,
            (StdModule::Env, "get") => EnvGet,
            (StdModule::Env, "set") => EnvSet,
            (StdModule::Env, "args") => EnvArgs,
            (StdModule::Nn, "tensor") => NnTensor,
            (StdModule::Nn, "zeros") => NnZeros,
            (StdModule::Nn, "ones") => NnOnes,
            (StdModule::Nn, "randn") => NnRandn,
            (StdModule::Nn, "rand") => NnRand,
            (StdModule::Nn, "arange") => NnArange,
            (StdModule::Nn, "eye") => NnEye,
            (StdModule::Nn, "param") => NnParam,
            (StdModule::Nn, "seed") => NnSeed,
            (StdModule::Nn, "sgd") => NnSgd,
            (StdModule::Nn, "adam") => NnAdam,
            (StdModule::Nn, "mse") => NnMse,
            (StdModule::Nn, "cross_entropy") => NnCrossEntropy,
            (StdModule::Nn, "stack") => NnStack,
            _ => return None,
        })
    }
}

impl Builtin {
    pub fn global(name: &str) -> Option<Builtin> {
        use Builtin::*;
        Some(match name {
            "len" => Len,
            "range" => Range,
            "str" => ToStr,
            "int" => ToInt,
            "float" => ToFloat,
            "bool" => ToBool,
            "abs" => Abs,
            "min" => Min,
            "max" => Max,
            "round" => Round,
            "assert" => Assert,
            "assert_eq" => AssertEq,
            "fail" => Fail,
            "type_of" => TypeOf,
            "input" => Input,
            _ => return None,
        })
    }

    pub fn is_constant(self) -> bool {
        matches!(self, Builtin::MathPi | Builtin::MathE)
    }

    pub fn name(self) -> &'static str {
        use Builtin::*;
        match self {
            Len => "len",
            Range => "range",
            ToStr => "str",
            ToInt => "int",
            ToFloat => "float",
            ToBool => "bool",
            Abs => "abs",
            Min => "min",
            Max => "max",
            Round => "round",
            Assert => "assert",
            AssertEq => "assert_eq",
            Fail => "fail",
            TypeOf => "type_of",
            Input => "input",
            MathPi => "math.pi",
            MathE => "math.e",
            MathSqrt => "math.sqrt",
            MathPow => "math.pow",
            MathFloor => "math.floor",
            MathCeil => "math.ceil",
            MathSin => "math.sin",
            MathCos => "math.cos",
            MathTan => "math.tan",
            MathLog => "math.log",
            MathExp => "math.exp",
            TimeNow => "time.now",
            TimeNowMs => "time.now_ms",
            TimeSleep => "time.sleep",
            JsonParse => "json.parse",
            JsonStringify => "json.stringify",
            FsRead => "fs.read_file",
            FsWrite => "fs.write_file",
            FsAppend => "fs.append_file",
            FsExists => "fs.exists",
            FsListDir => "fs.list_dir",
            FsDelete => "fs.delete_file",
            FsCreateDir => "fs.create_dir",
            EnvGet => "env.get",
            EnvSet => "env.set",
            EnvArgs => "env.args",
            NnTensor => "nn.tensor",
            NnZeros => "nn.zeros",
            NnOnes => "nn.ones",
            NnRandn => "nn.randn",
            NnRand => "nn.rand",
            NnArange => "nn.arange",
            NnEye => "nn.eye",
            NnParam => "nn.param",
            NnSeed => "nn.seed",
            NnSgd => "nn.sgd",
            NnAdam => "nn.adam",
            NnMse => "nn.mse",
            NnCrossEntropy => "nn.cross_entropy",
            NnStack => "nn.stack",
        }
    }

    /// Signature of builtins with a fixed shape: parameters, how many are required, and the result.
    pub fn fixed_signature(self) -> Option<(Vec<Ty>, usize, Ty)> {
        use Builtin::*;
        let s = || Ty::Str;
        let f = || Ty::Float;
        Some(match self {
            Assert => (vec![Ty::Bool, s()], 1, Ty::None),
            Input => (vec![s()], 0, s()),
            MathPi | MathE => (vec![], 0, f()),
            MathSqrt | MathSin | MathCos | MathTan | MathLog | MathExp => (vec![f()], 1, f()),
            MathPow => (vec![f(), f()], 2, f()),
            MathFloor | MathCeil => (vec![f()], 1, Ty::Int),
            TimeNow => (vec![], 0, f()),
            TimeNowMs => (vec![], 0, Ty::Int),
            TimeSleep => (vec![Ty::Int], 1, Ty::None),
            JsonParse => (vec![s()], 1, Ty::Any),
            FsRead => (vec![s()], 1, s()),
            FsWrite | FsAppend => (vec![s(), s()], 2, Ty::None),
            FsExists => (vec![s()], 1, Ty::Bool),
            FsListDir => (vec![s()], 1, Ty::list(s())),
            FsDelete | FsCreateDir => (vec![s()], 1, Ty::None),
            EnvGet => (vec![s(), s()], 1, s()),
            EnvSet => (vec![s(), s()], 2, Ty::None),
            EnvArgs => (vec![], 0, Ty::list(s())),
            NnTensor => (vec![Ty::Any], 1, Ty::Tensor),
            NnZeros | NnOnes | NnRandn | NnRand => (vec![Ty::list(Ty::Int)], 1, Ty::Tensor),
            NnArange | NnEye => (vec![Ty::Int], 1, Ty::Tensor),
            NnParam => (vec![Ty::Tensor], 1, Ty::Tensor),
            NnSeed => (vec![Ty::Int], 1, Ty::None),
            NnSgd | NnAdam => (vec![Ty::list(Ty::Tensor), f()], 2, Ty::Optimizer),
            NnMse | NnCrossEntropy => (vec![Ty::Tensor, Ty::Tensor], 2, Ty::Tensor),
            NnStack => (vec![Ty::list(Ty::Tensor), Ty::Int], 1, Ty::Tensor),
            _ => return None,
        })
    }
}

/// Signature of a method on a built-in type: parameters, how many are required, and the result.
pub fn method_signature(recv: &Ty, name: &str) -> Option<(Vec<Ty>, usize, Ty)> {
    let s = || Ty::Str;
    let i = || Ty::Int;
    let b = || Ty::Bool;
    match recv {
        Ty::Str => Some(match name {
            "length" | "len" => (vec![], 0, i()),
            "to_upper" | "upper" | "to_lower" | "lower" | "trim" | "trim_start" | "trim_end" => (vec![], 0, s()),
            "split" => (vec![s()], 0, Ty::list(s())),
            "lines" | "chars" => (vec![], 0, Ty::list(s())),
            "replace" => (vec![s(), s()], 2, s()),
            "contains" | "starts_with" | "ends_with" => (vec![s()], 1, b()),
            "index_of" => (vec![s()], 1, i()),
            "slice" => (vec![i(), i()], 1, s()),
            "repeat" => (vec![i()], 1, s()),
            "pad_start" | "pad_end" => (vec![i(), s()], 1, s()),
            "is_empty" => (vec![], 0, b()),
            _ => return None,
        }),
        Ty::List(t) => {
            let t = (**t).clone();
            Some(match name {
                "length" | "len" => (vec![], 0, i()),
                "push" | "append" => (vec![t], 1, Ty::None),
                "pop" => (vec![], 0, t),
                "insert" => (vec![i(), t], 2, Ty::None),
                "remove" => (vec![i()], 1, t),
                "contains" => (vec![t], 1, b()),
                "index_of" => (vec![t], 1, i()),
                "slice" => (vec![i(), i()], 1, Ty::list(t)),
                "reverse" | "sort" | "clear" => (vec![], 0, Ty::None),
                "join" => (vec![s()], 0, s()),
                "extend" => (vec![Ty::list(t)], 1, Ty::None),
                "copy" | "clone" => (vec![], 0, Ty::list(t)),
                "first" | "last" => (vec![], 0, t),
                "is_empty" => (vec![], 0, b()),
                _ => return None,
            })
        }
        Ty::Map(k, v) => {
            let (k, v) = ((**k).clone(), (**v).clone());
            Some(match name {
                "length" | "len" => (vec![], 0, i()),
                "has" | "contains" => (vec![k], 1, b()),
                "keys" => (vec![], 0, Ty::list(k)),
                "values" => (vec![], 0, Ty::list(v)),
                "get" => (vec![k, v.clone()], 2, v),
                "remove" => (vec![k], 1, v),
                "clear" => (vec![], 0, Ty::None),
                "copy" | "clone" => (vec![], 0, Ty::map(k, v)),
                "is_empty" => (vec![], 0, b()),
                _ => return None,
            })
        }
        Ty::Tensor => {
            let t = || Ty::Tensor;
            Some(match name {
                "shape" => (vec![], 0, Ty::list(i())),
                "rank" | "size" => (vec![], 0, i()),
                "item" => (vec![], 0, Ty::Float),
                "to_list" => (vec![], 0, Ty::Any),
                "sum" | "mean" => (vec![i()], 0, t()),
                "max" | "min" | "argmax" | "argmin" | "softmax" | "log_softmax" | "squeeze" | "unsqueeze" => (vec![i()], 1, t()),
                "relu" | "sigmoid" | "tanh" | "exp" | "log" | "sqrt" | "abs" | "gelu" | "silu" | "square" | "t" | "flatten" | "detach"
                | "grad" | "copy" => (vec![], 0, t()),
                "reshape" => (vec![Ty::list(i())], 1, t()),
                "transpose" => (vec![i(), i()], 2, t()),
                "matmul" | "eq" => (vec![t()], 1, t()),
                "pow" => (vec![Ty::Float], 1, t()),
                "clamp" => (vec![Ty::Float, Ty::Float], 2, t()),
                "backward" => (vec![], 0, Ty::None),
                _ => return None,
            })
        }
        Ty::Optimizer => Some(match name {
            "step" => (vec![Ty::Tensor], 1, Ty::None),
            "lr" => (vec![], 0, Ty::Float),
            "set_lr" => (vec![Ty::Float], 1, Ty::None),
            _ => return None,
        }),
        _ => None,
    }
}
