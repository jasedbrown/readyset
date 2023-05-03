use std::iter;

use nom_sql::{
    BinaryOperator as SqlBinaryOperator, Column, Expr as AstExpr, FunctionExpr, InValue, Relation,
    UnaryOperator,
};
use readyset_data::dialect::SqlEngine;
use readyset_data::{DfType, DfValue};
use readyset_errors::{
    internal, internal_err, invalid, invalid_err, unsupported, ReadySetError, ReadySetResult,
};
use readyset_util::redacted::Sensitive;
use vec1::Vec1;

use crate::{
    BinaryOperator, BuiltinFunction, CaseWhenBranch, Dialect, Expr, NullValueTreatmentArg,
};

/// Context supplied to expression lowering to allow resolving references to objects within the
/// schema
pub trait LowerContext: Clone {
    /// Look up a column in the parent node of the node containing this expression, returning its
    /// column index and type
    fn resolve_column(&self, col: Column) -> ReadySetResult<(usize, DfType)>;

    /// Look up a named custom type in the schema.
    fn resolve_type(&self, ty: Relation) -> Option<DfType>;
}

/// Unify the given list of types according to PostgreSQL's [type unification rules][pg-docs]
///
/// [pg-docs]: https://www.postgresql.org/docs/current/typeconv-union-case.html
fn unify_postgres_types(types: Vec<&DfType>) -> ReadySetResult<DfType> {
    let Some(first_ty) = types.first() else {
        return Ok(DfType::DEFAULT_TEXT)
    };

    // > 1. If all inputs are of the same type, and it is not unknown, resolve as that type.
    if types.iter().skip(1).all(|t| t == first_ty) && first_ty.is_known() {
        return Ok((*first_ty).clone());
    }

    // > 2. If any input is of a domain type, treat it as being of the domain's base type for all
    // > subsequent steps.
    //
    // We don't support domain types, so we can skip this step

    // > 3. If all inputs are of type unknown, resolve as type text (the preferred type of the
    // > string category). Otherwise, unknown inputs are ignored for the purposes of the remaining
    // > rules.
    let Some(first_known_type) = types.iter().find(|t| t.is_known()) else {
        return Ok(DfType::DEFAULT_TEXT);
    };

    // > 4. If the non-unknown inputs are not all of the same type category, fail.
    if types
        .iter()
        .skip(1)
        .filter(|t| t.is_known())
        .any(|t| t.pg_category() != first_known_type.pg_category())
    {
        invalid_err!(
            "Cannot coerce type {} to type {}",
            first_ty,
            types
                .get(1)
                .expect("can't get here unless we have at least 2 types")
        );
    }

    // > 5. Select the first non-unknown input type as the candidate type, then consider each other
    // > non-unknown input type, left to right. [13] If the candidate type can be implicitly
    // > converted to the other type, but not vice-versa, select the other type as the new candidate
    // > type. Then continue considering the remaining inputs. If, at any stage of this process, a
    // > preferred type is selected, stop considering additional inputs.
    //
    // TODO(ENG-1909): This bit is not completely correct, because we haven't yet implemented the
    // notion of "preferred" types, or whether or not types can be implicitly converted. For now, we
    // just take the first non-unknown type
    Ok((*first_known_type).clone())
}

/// Returns a tuple of the inferred type to convert arguments to for comparison within a call to
/// `GREATEST` or `LEAST` using MySQL's [inference rules for those functions][mysql-docs]
///
/// [mysql-docs]: https://dev.mysql.com/doc/refman/8.0/en/comparison-operators.html#function_least
fn mysql_least_greatest_compare_as(arg_types: Vec<&DfType>) -> DfType {
    // > * If any argument is NULL, the result is NULL. No comparison is needed.
    if arg_types.is_empty() || arg_types.iter().any(|t| t.is_unknown()) {
        return DfType::Unknown;
    }

    // > * If all arguments are integer-valued, they are compared as integers.
    if arg_types.iter().all(|t| t.is_any_int()) {
        return DfType::BigInt;
    }

    // > * If at least one argument is double precision, they are compared as double-precision
    // > values
    if arg_types.iter().any(|t| t.is_any_float()) {
        return DfType::Double;
    }

    // > * If the arguments comprise a mix of numbers and strings, they are compared as strings.
    // > * If any argument is a nonbinary (character) string, the arguments are compared as
    // > nonbinary
    // > strings.
    //
    // As far as I can tell, these two bullet points mean the same thing...
    let mut has_strings = false;
    let mut has_numbers = false;
    for arg_ty in &arg_types {
        if arg_ty.is_any_text() {
            has_strings = true;
        } else if arg_ty.is_any_int() {
            has_numbers = true;
        }

        if has_strings && has_numbers {
            return DfType::DEFAULT_TEXT;
        }
    }

    // > * In all other cases, the arguments are compared as binary strings.
    DfType::VarBinary(u16::MAX)
}

impl BuiltinFunction {
    pub(crate) fn from_name_and_args<A>(
        name: &str,
        args: A,
        dialect: Dialect,
    ) -> Result<(Self, DfType), ReadySetError>
    where
        A: IntoIterator<Item = Expr>,
    {
        fn type_for_round(expr: &Expr, _precision: &Expr) -> DfType {
            use DfType::*;
            match *expr.ty() {
                Unknown => Unknown,

                // When the first argument is a DECIMAL value, the return type is also DECIMAL.
                Numeric { prec, scale } => Numeric { prec, scale },

                // When the first argument is of any integer type, the return type is always BIGINT.
                ref ty if ty.is_any_int() => BigInt,

                // When the first argument is of any floating-point or non-numeric type, the return
                // type is always DOUBLE.
                _ => Double,
            }
        }

        let arity_error = || ReadySetError::ArityError(name.to_owned());

        // TODO: Type-check arguments.
        let mut args = args.into_iter();
        let mut next_arg = || args.next().ok_or_else(arity_error);
        let cast = |expr: Expr, ty| {
            if *expr.ty() == ty {
                // optimization: don't cast if the expr already has the target type
                expr
            } else {
                Expr::Cast {
                    expr: Box::new(expr),
                    ty,
                    null_on_failure: false,
                }
            }
        };
        let try_cast = |expr, ty| Expr::Cast {
            expr: Box::new(expr),
            ty,
            null_on_failure: true,
        };

        let result = match name {
            "convert_tz" => {
                // Type is inferred from input argument
                let input = next_arg()?;
                let ty = input.ty().clone();
                (
                    Self::ConvertTZ([
                        try_cast(
                            input,
                            DfType::Timestamp {
                                subsecond_digits: ty
                                    .subsecond_digits()
                                    .unwrap_or_else(|| dialect.default_subsecond_digits()),
                            },
                        ),
                        cast(next_arg()?, DfType::DEFAULT_TEXT),
                        cast(next_arg()?, DfType::DEFAULT_TEXT),
                    ]),
                    ty,
                )
            }
            "dayofweek" => {
                (
                    Self::DayOfWeek(try_cast(next_arg()?, DfType::Date)),
                    DfType::Int, // Day of week is always an int
                )
            }
            "ifnull" => {
                let expr = next_arg()?;
                let val = next_arg()?;
                // Type is inferred from the value provided
                let ty = val.ty().clone();
                (Self::IfNull(expr, val), ty)
            }
            "month" => {
                (
                    Self::Month(try_cast(next_arg()?, DfType::Date)),
                    DfType::Int, // Month is always an int
                )
            }
            "timediff" => {
                (
                    Self::Timediff(next_arg()?, next_arg()?),
                    // type is always time
                    DfType::Time {
                        subsecond_digits: dialect.default_subsecond_digits(),
                    },
                )
            }
            "addtime" => {
                let base_time = next_arg()?;
                let ty = base_time.ty().clone();
                (Self::Addtime(base_time, next_arg()?), ty)
            }
            "date_format" => (
                Self::DateFormat(next_arg()?, next_arg()?),
                DfType::DEFAULT_TEXT,
            ),
            "round" => {
                let expr = next_arg()?;
                let prec = args.next().unwrap_or(Expr::Literal {
                    val: DfValue::Int(0),
                    ty: DfType::Int,
                });
                let ty = type_for_round(&expr, &prec);
                (Self::Round(expr, prec), ty)
            }
            "json_depth" => (Self::JsonDepth(next_arg()?), DfType::Int),
            "json_valid" => (Self::JsonValid(next_arg()?), DfType::BigInt),
            "json_overlaps" => (Self::JsonOverlaps(next_arg()?, next_arg()?), DfType::BigInt),
            "json_quote" => (Self::JsonQuote(next_arg()?), DfType::DEFAULT_TEXT),
            "json_typeof" | "jsonb_typeof" => (
                Self::JsonTypeof(next_arg()?),
                // Always returns text containing the JSON type.
                DfType::DEFAULT_TEXT,
            ),
            "json_object" => match dialect.engine() {
                // TODO(ENG-1536): https://dev.mysql.com/doc/refman/8.0/en/json-creation-functions.html#function_json-object
                SqlEngine::MySQL => unsupported!("MySQL 'json_object' not yet supported"),
                SqlEngine::PostgreSQL => (
                    Self::JsonObject {
                        arg1: next_arg()?,
                        arg2: args.next(),
                        allow_duplicate_keys: true,
                    },
                    DfType::Json,
                ),
                SqlEngine::MongoDB => unsupported!("not yet supported"),
            },
            "jsonb_object" => (
                Self::JsonObject {
                    arg1: next_arg()?,
                    arg2: args.next(),
                    allow_duplicate_keys: false,
                },
                DfType::Jsonb,
            ),
            "json_array_length" | "jsonb_array_length" => {
                (Self::JsonArrayLength(next_arg()?), DfType::Int)
            }
            "json_strip_nulls" => (Self::JsonStripNulls(next_arg()?), DfType::Json),
            "jsonb_strip_nulls" => (Self::JsonStripNulls(next_arg()?), DfType::Jsonb),
            "json_extract_path" => (
                Self::JsonExtractPath {
                    json: next_arg()?,
                    keys: Vec1::try_from_vec(args.by_ref().collect()).map_err(|_| arity_error())?,
                },
                DfType::Json,
            ),
            "jsonb_extract_path" => (
                Self::JsonExtractPath {
                    json: next_arg()?,
                    keys: Vec1::try_from_vec(args.by_ref().collect()).map_err(|_| arity_error())?,
                },
                DfType::Jsonb,
            ),

            "json_extract_path_text" | "jsonb_extract_path_text" => (
                Self::JsonExtractPath {
                    json: next_arg()?,
                    keys: Vec1::try_from_vec(args.by_ref().collect()).map_err(|_| arity_error())?,
                },
                DfType::DEFAULT_TEXT,
            ),
            "jsonb_insert" => (
                Self::JsonbInsert(next_arg()?, next_arg()?, next_arg()?, args.next()),
                DfType::Jsonb,
            ),
            "jsonb_set" => (
                Self::JsonbSet(
                    next_arg()?,
                    next_arg()?,
                    next_arg()?,
                    args.next(),
                    NullValueTreatmentArg::ReturnNull,
                ),
                DfType::Jsonb,
            ),
            "jsonb_set_lax" => (
                Self::JsonbSet(
                    next_arg()?,
                    next_arg()?,
                    next_arg()?,
                    args.next(),
                    NullValueTreatmentArg::Expr(args.next()),
                ),
                DfType::Jsonb,
            ),
            "jsonb_pretty" => (Self::JsonbPretty(next_arg()?), DfType::DEFAULT_TEXT),
            "coalesce" => {
                let arg1 = next_arg()?;
                let ty = arg1.ty().clone();
                (Self::Coalesce(arg1, args.by_ref().collect()), ty)
            }
            "concat" => {
                let arg1 = next_arg()?;
                let rest_args = args.by_ref().collect::<Vec<_>>();
                let collation = iter::once(&arg1)
                    .chain(&rest_args)
                    .find_map(|expr| match expr.ty() {
                        DfType::Text(c) => Some(*c),
                        _ => None,
                    })
                    .unwrap_or_default();
                let ty = DfType::Text(collation);
                (
                    Self::Concat(
                        cast(arg1, ty.clone()),
                        rest_args
                            .into_iter()
                            .map(|arg| cast(arg, ty.clone()))
                            .collect(),
                    ),
                    ty,
                )
            }
            "substring" | "substr" => {
                let string = next_arg()?;
                let ty = if string.ty().is_any_text() {
                    string.ty().clone()
                } else {
                    DfType::DEFAULT_TEXT
                };

                (
                    Self::Substring(
                        cast(string, ty.clone()),
                        next_arg().ok().map(|arg| cast(arg, DfType::BigInt)),
                        next_arg().ok().map(|arg| cast(arg, DfType::BigInt)),
                    ),
                    ty,
                )
            }
            "split_part" => (
                Self::SplitPart(
                    cast(next_arg()?, DfType::DEFAULT_TEXT),
                    cast(next_arg()?, DfType::DEFAULT_TEXT),
                    cast(next_arg()?, DfType::Int),
                ),
                DfType::DEFAULT_TEXT,
            ),
            "greatest" | "least" => {
                // The type inference rules for GREATEST and LEAST are the same, so this block
                // covers both then dispatches for the actual function construction at the end
                let arg1 = next_arg()?;
                let rest_args = args.by_ref().collect::<Vec<_>>();
                let arg_tys = iter::once(arg1.ty())
                    .chain(rest_args.iter().map(|arg| arg.ty()))
                    .collect::<Vec<_>>();
                let (compare_as, ty) = match dialect.engine() {
                    SqlEngine::PostgreSQL => {
                        let ty = unify_postgres_types(arg_tys)?;
                        (ty.clone(), ty)
                    }
                    SqlEngine::MySQL => {
                        // TODO(ENG-1911): What are the rules for MySQL's return type inference?
                        // The documentation just says "The return type of LEAST() is the aggregated
                        // type of the comparison argument types."
                        let return_ty = arg_tys
                            .iter()
                            .find(|t| t.is_known())
                            .copied()
                            .cloned()
                            .unwrap_or(DfType::Binary(0));
                        let compare_as = mysql_least_greatest_compare_as(arg_tys);
                        (compare_as, return_ty)
                    }
                    SqlEngine::MongoDB => unsupported!("not supported")
                };

                let mut args = Vec1::with_capacity(arg1, rest_args.len() + 1);
                args.extend(rest_args);

                (
                    if name == "greatest" {
                        Self::Greatest { args, compare_as }
                    } else {
                        Self::Least { args, compare_as }
                    },
                    ty,
                )
            }
            "array_to_string" => {
                let array_arg = next_arg()?;
                let elem_ty = match array_arg.ty() {
                    DfType::Array(t) => (**t).clone(),
                    _ => DfType::Unknown,
                };
                (
                    Self::ArrayToString(
                        cast(array_arg, DfType::Array(Box::new(elem_ty))),
                        next_arg()?,
                        next_arg().ok(),
                    ),
                    DfType::DEFAULT_TEXT,
                )
            }
            _ => return Err(ReadySetError::NoSuchFunction(name.to_owned())),
        };

        if args.next().is_some() {
            return Err(arity_error());
        }

        Ok(result)
    }
}

impl Expr {
    /// Lower the given [`nom_sql`] AST expression to a dataflow expression
    ///
    /// Currently, this involves:
    ///
    /// - Literals being replaced with their corresponding [`DfValue`]
    /// - [Column references](nom_sql::Column) being resolved into column indices in the parent
    ///   node.
    /// - Function calls being resolved to built-in functions, and arities checked
    /// - Desugaring x IN (y, z, ...) to `x = y OR x = z OR ...` and x NOT IN (y, z, ...) to `x != y
    ///   AND x != z AND ...`
    /// - Replacing unary negation with `(expr * -1)`
    /// - Replacing unary NOT with `(expr != 1)`
    /// - Inferring the type of each node in the expression AST.
    pub fn lower<C>(expr: AstExpr, dialect: Dialect, context: C) -> ReadySetResult<Self>
    where
        C: LowerContext,
    {
        match expr {
            AstExpr::Call(FunctionExpr::Call {
                name: fname,
                arguments,
            }) => {
                let args = arguments
                    .into_iter()
                    .map(|arg| Self::lower(arg, dialect, context.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                let (func, ty) = BuiltinFunction::from_name_and_args(&fname, args, dialect)?;
                Ok(Self::Call {
                    func: Box::new(func),
                    ty,
                })
            }
            AstExpr::Call(FunctionExpr::Substring { string, pos, len }) => {
                let string = Self::lower(*string, dialect, context.clone())?;
                let ty = if string.ty().is_any_text() {
                    string.ty().clone()
                } else {
                    DfType::DEFAULT_TEXT
                };
                let func = Box::new(BuiltinFunction::Substring(
                    string,
                    pos.map(|expr| Self::lower(*expr, dialect, context.clone()))
                        .transpose()?,
                    len.map(|expr| Self::lower(*expr, dialect, context))
                        .transpose()?,
                ));

                Ok(Self::Call { func, ty })
            }
            AstExpr::Call(call) => internal!(
                "Unexpected (aggregate?) call node in project expression: {:?}",
                Sensitive(&call)
            ),
            AstExpr::Literal(lit) => {
                let is_string_literal = lit.is_string();
                let val: DfValue = lit.try_into()?;
                // TODO: Infer type from SQL
                let ty = if is_string_literal && dialect.engine() == SqlEngine::PostgreSQL {
                    DfType::Unknown
                } else {
                    val.infer_dataflow_type()
                };

                Ok(Self::Literal { val, ty })
            }
            AstExpr::Column(col) => {
                let (index, ty) = context.resolve_column(col)?;
                Ok(Self::Column { index, ty })
            }
            AstExpr::BinaryOp { lhs, op, rhs } => {
                let mut left = Box::new(Self::lower(*lhs, dialect, context.clone())?);
                let mut right = Box::new(Self::lower(*rhs, dialect, context)?);
                let (op, negated) =
                    BinaryOperator::from_sql_op(op, dialect, left.ty(), right.ty())?;

                if matches!(
                    op,
                    BinaryOperator::JsonPathExtract | BinaryOperator::JsonPathExtractUnquote
                ) {
                    unsupported!("'{op}' operator not implemented yet for MySQL");
                }

                let ty = op.output_type(left.ty(), right.ty())?;
                let (left_coerce_target, right_coerce_target) =
                    op.argument_type_coercions(left.ty(), right.ty())?;

                if let Some(ty) = left_coerce_target {
                    left = Box::new(Self::Cast {
                        expr: left,
                        ty,
                        null_on_failure: false,
                    })
                }
                if let Some(ty) = right_coerce_target {
                    right = Box::new(Self::Cast {
                        expr: right,
                        ty,
                        null_on_failure: false,
                    })
                }

                let op_node = Self::Op {
                    op,
                    left,
                    right,
                    ty,
                };

                if negated {
                    Ok(Self::Not {
                        expr: Box::new(op_node),
                        ty: DfType::Bool,
                    })
                } else {
                    Ok(op_node)
                }
            }
            AstExpr::OpAny { lhs, op, rhs } | AstExpr::OpSome { lhs, op, rhs } => {
                Self::lower_op_any_or_all(*lhs, op, *rhs, dialect, context, false)
            }
            AstExpr::OpAll { lhs, op, rhs } => {
                Self::lower_op_any_or_all(*lhs, op, *rhs, dialect, context, true)
            }
            AstExpr::UnaryOp {
                op: UnaryOperator::Neg,
                rhs,
            } => {
                let left = Box::new(Self::lower(*rhs, dialect, context)?);
                // TODO: Negation may change type to signed
                let ty = left.ty().clone();
                Ok(Self::Op {
                    op: BinaryOperator::Multiply,
                    left,
                    right: Box::new(Self::Literal {
                        val: DfValue::Int(-1),
                        ty: DfType::Int,
                    }),
                    ty,
                })
            }
            AstExpr::UnaryOp {
                op: UnaryOperator::Not,
                rhs,
            } => Ok(Self::Not {
                expr: Box::new(Self::lower(*rhs, dialect, context)?),
                ty: DfType::Bool, // type of NOT is always bool
            }),
            AstExpr::Cast {
                expr, ty: to_type, ..
            } => {
                let ty = DfType::from_sql_type(&to_type, dialect, |t| context.resolve_type(t))?;
                Ok(Self::Cast {
                    expr: Box::new(Self::lower(*expr, dialect, context)?),
                    ty,
                    null_on_failure: false,
                })
            }
            AstExpr::CaseWhen {
                branches,
                else_expr,
            } => {
                let branches = branches
                    .into_iter()
                    .map(|branch| {
                        let condition = Self::lower(branch.condition, dialect, context.clone())?;
                        let body = Self::lower(branch.body, dialect, context.clone())?;
                        Ok(CaseWhenBranch { condition, body })
                    })
                    .collect::<ReadySetResult<Vec<_>>>()?;
                // TODO: Do case arm types have to match? See if type is inferred at runtime
                let ty = branches
                    .first()
                    .ok_or_else(|| internal_err!("CASE expression cannot have zero branches"))?
                    .body
                    .ty()
                    .clone();
                Ok(Self::CaseWhen {
                    branches,
                    else_expr: match else_expr {
                        Some(else_expr) => Box::new(Self::lower(*else_expr, dialect, context)?),
                        None => Box::new(Self::Literal {
                            val: DfValue::None,
                            ty: DfType::Unknown,
                        }),
                    },
                    ty,
                })
            }
            AstExpr::In {
                lhs,
                rhs: InValue::List(exprs),
                negated,
            } => {
                let mut exprs = exprs.into_iter();
                if let Some(fst) = exprs.next() {
                    let logical_op = if negated {
                        BinaryOperator::And
                    } else {
                        BinaryOperator::Or
                    };

                    let lhs = Self::lower(*lhs, dialect, context.clone())?;
                    let make_comparison = |rhs| -> ReadySetResult<_> {
                        let equal = Self::Op {
                            left: Box::new(lhs.clone()),
                            op: BinaryOperator::Equal,
                            right: Box::new(Self::lower(rhs, dialect, context.clone())?),
                            ty: DfType::Bool, // type of = is always bool
                        };
                        if negated {
                            Ok(Self::Not {
                                expr: Box::new(equal),
                                ty: DfType::Bool,
                            })
                        } else {
                            Ok(equal)
                        }
                    };

                    exprs.try_fold(make_comparison(fst)?, |acc, rhs| {
                        Ok(Self::Op {
                            left: Box::new(acc),
                            op: logical_op,
                            right: Box::new(make_comparison(rhs)?),
                            ty: DfType::Bool, // type of and/or is always bool
                        })
                    })
                } else if negated {
                    // x IN () is always false
                    Ok(Self::Literal {
                        val: DfValue::None,
                        ty: DfType::Bool,
                    })
                } else {
                    // x NOT IN () is always false
                    Ok(Self::Literal {
                        val: DfValue::from(1),
                        ty: DfType::Bool,
                    })
                }
            }
            expr @ AstExpr::Array(_) => {
                fn find_shape(expr: &AstExpr, out: &mut Vec<usize>) {
                    if let AstExpr::Array(elems) = expr {
                        out.push(elems.len());
                        if let Some(elem) = elems.first() {
                            find_shape(elem, out)
                        }
                    }
                }

                fn flatten<C>(
                    expr: AstExpr,
                    out: &mut Vec<Expr>,
                    dialect: Dialect,
                    context: C,
                ) -> ReadySetResult<()>
                where
                    C: LowerContext,
                {
                    match expr {
                        AstExpr::Array(exprs) => {
                            for expr in exprs {
                                flatten(expr, out, dialect, context.clone())?;
                            }
                        }
                        _ => out.push(Expr::lower(expr, dialect, context)?),
                    }
                    Ok(())
                }

                let mut shape = vec![];
                let mut elements = vec![];
                find_shape(&expr, &mut shape);
                flatten(expr, &mut elements, dialect, context)?;

                let mut ty =
                    // Array exprs are only supported for postgresql
                    unify_postgres_types(elements.iter().map(|expr| expr.ty()).collect())?;

                for _ in &shape {
                    ty = DfType::Array(Box::new(ty));
                }

                Ok(Self::Array {
                    elements,
                    shape,
                    ty,
                })
            }
            AstExpr::Exists(_) => unsupported!("EXISTS not currently supported"),
            AstExpr::Variable(_) => unsupported!("Variables not currently supported"),
            AstExpr::Between { .. } | AstExpr::NestedSelect(_) | AstExpr::In { .. } => {
                internal!(
                    "Expression should have been desugared earlier: {}",
                    expr.display(nom_sql::Dialect::MySQL)
                )
            }
        }
    }

    fn lower_op_any_or_all<C>(
        lhs: AstExpr,
        op: SqlBinaryOperator,
        rhs: AstExpr,
        dialect: Dialect,
        context: C,
        mut is_all: bool,
    ) -> ReadySetResult<Expr>
    where
        C: LowerContext,
    {
        let mut left = Box::new(Self::lower(lhs, dialect, context.clone())?);
        let mut right = Box::new(Self::lower(rhs, dialect, context)?);
        let (op, negated) = BinaryOperator::from_sql_op(op, dialect, left.ty(), right.ty())?;
        if negated {
            is_all = !is_all
        }

        let right_member_ty = if right.ty().is_array() {
            right.ty().innermost_array_type()
        } else if right.ty().is_unknown() {
            &DfType::Unknown
        } else {
            // localhost/postgres=# select 1 = any(1);
            // ERROR:  42809: op ANY/ALL (array) requires array on right side
            // LINE 1: select 1 = any(1);
            //                  ^
            // LOCATION:  make_scalar_array_op, parse_oper.c:814
            // Time: 0.396 ms
            invalid!("op ANY/ALL (array) requires an array on the right-hand side")
        };

        let ty = op.output_type(left.ty(), right_member_ty)?;
        if !ty.is_bool() {
            // localhost/noria=# select 1 + any('{1,2}');
            // ERROR:  42809: op ANY/ALL (array) requires operator to yield boolean
            // LINE 1: select 1 + any('{1,2}');
            //                  ^
            // LOCATION:  make_scalar_array_op, parse_oper.c:856
            // Time: 0.330 ms
            invalid!("op ANY/ALL (array) requires the operator to yield a boolean")
        }

        let (left_coerce_target, right_coerce_target) =
            op.argument_type_coercions(left.ty(), right_member_ty)?;

        if let Some(ty) = left_coerce_target {
            left = Box::new(Self::Cast {
                expr: left,
                ty,
                null_on_failure: false,
            })
        }
        if let Some(ty) = right_coerce_target {
            right = Box::new(Self::Cast {
                expr: right,
                ty: DfType::Array(Box::new(ty)),

                null_on_failure: false,
            })
        } else if !right.ty().is_array() {
            // Even if we don't need to cast the right member type to a target type, we still need
            // to cast the rhs to an array (but we can make it an array of UNKNOWN,
            // since we can leave the values alone)
            right = Box::new(Self::Cast {
                expr: right,
                ty: DfType::Array(Box::new(right_coerce_target.unwrap_or(DfType::Unknown))),
                null_on_failure: false,
            });
        }

        let op_node = if is_all {
            Self::OpAll {
                op,
                left,
                right,
                ty,
            }
        } else {
            Self::OpAny {
                op,
                left,
                right,
                ty,
            }
        };

        if negated {
            Ok(Self::Not {
                expr: Box::new(op_node),
                ty: DfType::Bool,
            })
        } else {
            Ok(op_node)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use nom_sql::{
        parse_expr, BinaryOperator as AstBinaryOperator, Dialect as ParserDialect, Float, Literal,
    };
    use readyset_data::{Collation, PgEnumMetadata};

    use super::*;

    #[derive(Clone)]
    pub(crate) struct TestLowerContext<RC, RT> {
        resolve_column: RC,
        resolve_type: RT,
    }

    impl<RC, RT> LowerContext for TestLowerContext<RC, RT>
    where
        RC: Fn(Column) -> ReadySetResult<(usize, DfType)> + Clone,
        RT: Fn(Relation) -> Option<DfType> + Clone,
    {
        fn resolve_column(&self, col: Column) -> ReadySetResult<(usize, DfType)> {
            (self.resolve_column)(col)
        }

        fn resolve_type(&self, ty: Relation) -> Option<DfType> {
            (self.resolve_type)(ty)
        }
    }

    pub(crate) fn no_op_lower_context() -> impl LowerContext {
        TestLowerContext {
            resolve_column: |_| internal!(),
            resolve_type: |_| None,
        }
    }

    pub(crate) fn resolve_columns<F>(resolve_column: F) -> impl LowerContext
    where
        F: Fn(Column) -> ReadySetResult<(usize, DfType)> + Clone,
    {
        TestLowerContext {
            resolve_column,
            resolve_type: |_| None,
        }
    }

    pub(crate) fn resolve_types<F>(resolve_type: F) -> impl LowerContext
    where
        F: Fn(Relation) -> Option<DfType> + Clone,
    {
        TestLowerContext {
            resolve_column: |_| internal!(),
            resolve_type,
        }
    }

    #[test]
    fn postgresql_text_literaal() {
        // localhost/postgres=# select pg_typeof('abc');
        //  pg_typeof
        // -----------
        //  unknown

        let input = AstExpr::Literal("abc".into());
        let result =
            Expr::lower(input, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
        assert_eq!(result.ty(), &DfType::Unknown);
    }

    #[test]
    fn simple_column_reference() {
        let input = AstExpr::Column("t.x".into());
        let result = Expr::lower(
            input,
            Dialect::DEFAULT_MYSQL,
            resolve_columns(|c| {
                if c == "t.x".into() {
                    Ok((0, DfType::Int))
                } else {
                    internal!("what's this column!?")
                }
            }),
        )
        .unwrap();
        assert_eq!(
            result,
            Expr::Column {
                index: 0,
                ty: DfType::Int
            }
        );
    }

    #[test]
    fn cast_to_custom_type() {
        let input =
            parse_expr(ParserDialect::PostgreSQL, "cast('foo' as something.custom)").unwrap();
        let enum_ty = DfType::from_enum_variants(
            ["foo".into(), "bar".into()],
            Some(PgEnumMetadata {
                name: "custom".into(),
                schema: "something".into(),
                oid: 12345,
                array_oid: 12344,
            }),
        );
        let result = Expr::lower(
            input,
            Dialect::DEFAULT_POSTGRESQL,
            resolve_types(|ty| {
                if ty.schema == Some("something".into()) && ty.name == "custom" {
                    Some(enum_ty.clone())
                } else {
                    None
                }
            }),
        )
        .unwrap();
        assert_eq!(
            result,
            Expr::Cast {
                expr: Box::new(Expr::Literal {
                    val: "foo".into(),
                    ty: DfType::Unknown
                }),
                ty: enum_ty,
                null_on_failure: false
            }
        );
    }

    #[test]
    fn call_coalesce() {
        let input = AstExpr::Call(FunctionExpr::Call {
            name: "coalesce".into(),
            arguments: vec![AstExpr::Column("t.x".into()), AstExpr::Literal(2.into())],
        });

        let result = Expr::lower(
            input,
            Dialect::DEFAULT_MYSQL,
            resolve_columns(|c| {
                if c == "t.x".into() {
                    Ok((0, DfType::Int))
                } else {
                    internal!("what's this column!?")
                }
            }),
        )
        .unwrap();

        assert_eq!(
            result,
            Expr::Call {
                func: Box::new(BuiltinFunction::Coalesce(
                    Expr::Column {
                        index: 0,
                        ty: DfType::Int
                    },
                    vec![Expr::Literal {
                        val: 2.into(),
                        ty: DfType::BigInt
                    }]
                )),
                ty: DfType::Int
            }
        );
    }

    #[test]
    fn call_concat_with_texts() {
        let input = parse_expr(ParserDialect::MySQL, "concat('My', 'SQ', 'L')").unwrap();
        let res = Expr::lower(input, Dialect::DEFAULT_MYSQL, no_op_lower_context()).unwrap();
        assert_eq!(
            res,
            Expr::Call {
                func: Box::new(BuiltinFunction::Concat(
                    Expr::Literal {
                        val: "My".into(),
                        ty: DfType::DEFAULT_TEXT,
                    },
                    vec![
                        Expr::Literal {
                            val: "SQ".into(),
                            ty: DfType::DEFAULT_TEXT,
                        },
                        Expr::Literal {
                            val: "L".into(),
                            ty: DfType::DEFAULT_TEXT,
                        },
                    ],
                )),
                ty: DfType::DEFAULT_TEXT,
            }
        );
    }

    #[test]
    fn substring_from_for() {
        let input = parse_expr(ParserDialect::MySQL, "substr(col from 1 for 7)").unwrap();
        let res = Expr::lower(
            input,
            Dialect::DEFAULT_MYSQL,
            resolve_columns(|c| {
                if c == "col".into() {
                    Ok((0, DfType::Text(Collation::Citext)))
                } else {
                    internal!()
                }
            }),
        )
        .unwrap();
        assert_eq!(
            res,
            Expr::Call {
                func: Box::new(BuiltinFunction::Substring(
                    Expr::Column {
                        index: 0,
                        ty: DfType::Text(Collation::Citext)
                    },
                    Some(Expr::Literal {
                        val: 1u32.into(),
                        ty: DfType::UnsignedBigInt
                    }),
                    Some(Expr::Literal {
                        val: 7u32.into(),
                        ty: DfType::UnsignedBigInt
                    })
                )),
                ty: DfType::Text(Collation::Citext)
            }
        )
    }

    #[test]
    fn substr_regular() {
        let input = parse_expr(ParserDialect::MySQL, "substr('abcdefghi', 1, 7)").unwrap();
        let res = Expr::lower(input, Dialect::DEFAULT_MYSQL, no_op_lower_context()).unwrap();
        assert_eq!(
            res,
            Expr::Call {
                func: Box::new(BuiltinFunction::Substring(
                    Expr::Literal {
                        val: "abcdefghi".into(),
                        ty: DfType::DEFAULT_TEXT
                    },
                    Some(Expr::Cast {
                        expr: Box::new(Expr::Literal {
                            val: 1.into(),
                            ty: DfType::UnsignedBigInt
                        }),
                        ty: DfType::BigInt,
                        null_on_failure: false
                    }),
                    Some(Expr::Cast {
                        expr: Box::new(Expr::Literal {
                            val: 7.into(),
                            ty: DfType::UnsignedBigInt
                        }),

                        ty: DfType::BigInt,
                        null_on_failure: false
                    })
                )),
                ty: DfType::DEFAULT_TEXT
            }
        )
    }

    #[test]
    fn substring_regular() {
        let input = parse_expr(ParserDialect::MySQL, "substring('abcdefghi', 1, 7)").unwrap();
        let res = Expr::lower(input, Dialect::DEFAULT_MYSQL, no_op_lower_context()).unwrap();
        assert_eq!(
            res,
            Expr::Call {
                func: Box::new(BuiltinFunction::Substring(
                    Expr::Literal {
                        val: "abcdefghi".into(),
                        ty: DfType::DEFAULT_TEXT
                    },
                    Some(Expr::Cast {
                        expr: Box::new(Expr::Literal {
                            val: 1.into(),
                            ty: DfType::UnsignedBigInt
                        }),
                        ty: DfType::BigInt,
                        null_on_failure: false
                    }),
                    Some(Expr::Cast {
                        expr: Box::new(Expr::Literal {
                            val: 7.into(),
                            ty: DfType::UnsignedBigInt
                        }),

                        ty: DfType::BigInt,
                        null_on_failure: false
                    })
                )),
                ty: DfType::DEFAULT_TEXT
            }
        )
    }

    #[test]
    fn substring_without_string_arg() {
        let input = parse_expr(ParserDialect::MySQL, "substring(123 from 2)").unwrap();
        let res = Expr::lower(input, Dialect::DEFAULT_MYSQL, no_op_lower_context()).unwrap();
        assert_eq!(res.ty(), &DfType::DEFAULT_TEXT);
    }

    #[test]
    fn greatest_inferred_type() {
        use Literal::Null;

        #[track_caller]
        fn infers_type(args: Vec<Literal>, dialect: Dialect, expected_ty: DfType) {
            let input = AstExpr::Call(FunctionExpr::Call {
                name: "greatest".into(),
                arguments: args.into_iter().map(AstExpr::Literal).collect(),
            });
            let result = Expr::lower(input, dialect, no_op_lower_context()).unwrap();
            assert_eq!(result.ty(), &expected_ty);
        }

        infers_type(
            vec![Null, Null],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::DEFAULT_TEXT,
        );

        infers_type(
            vec!["123".into(), 23.into()],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::BigInt,
        );

        infers_type(
            vec!["123".into(), "456".into()],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::DEFAULT_TEXT,
        );

        infers_type(
            vec!["123".into(), "456".into()],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::DEFAULT_TEXT,
        );

        infers_type(
            vec![
                "123".into(),
                Literal::Float(Float {
                    value: 1.23,
                    precision: 2,
                }),
            ],
            Dialect::DEFAULT_POSTGRESQL,
            // TODO: should actually be DfType::Numeric { prec: 10, scale: 0 }, but our type
            // inference for float literals in pg is (currently) wrong
            DfType::Float,
        );

        infers_type(
            vec![Null, 23.into()],
            Dialect::DEFAULT_MYSQL,
            DfType::BigInt,
        );
        infers_type(vec![Null, Null], Dialect::DEFAULT_MYSQL, DfType::Binary(0));

        infers_type(
            vec![
                "123".into(),
                Literal::Float(Float {
                    value: 1.23,
                    precision: 2,
                }),
            ],
            Dialect::DEFAULT_MYSQL,
            DfType::DEFAULT_TEXT,
        );

        // TODO(ENG-1911)
        // infers_type(
        //     vec![
        //         123.into(),
        //         Literal::Float(Float {
        //             value: 1.23,
        //             precision: 2,
        //         }),
        //     ],
        //     MySQL,
        //     DfType::Numeric { prec: 5, scale: 2 },
        // );
    }

    #[test]
    fn greatest_compare_as() {
        use Literal::Null;

        #[track_caller]
        fn compares_as(args: Vec<Literal>, dialect: Dialect, expected_ty: DfType) {
            let input = AstExpr::Call(FunctionExpr::Call {
                name: "greatest".into(),
                arguments: args.into_iter().map(AstExpr::Literal).collect(),
            });
            let result = Expr::lower(input, dialect, no_op_lower_context()).unwrap();
            let compare_as = match result {
                Expr::Call { func, .. } => match *func {
                    BuiltinFunction::Greatest { compare_as, .. } => compare_as,
                    _ => panic!("Wrong function"),
                },
                _ => panic!("Not a function call"),
            };
            assert_eq!(compare_as, expected_ty);
        }

        compares_as(
            vec![Null, Null],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::DEFAULT_TEXT,
        );
        compares_as(vec![Null, Null], Dialect::DEFAULT_MYSQL, DfType::Unknown);
        compares_as(
            vec![Null, "a".into(), "b".into()],
            Dialect::DEFAULT_MYSQL,
            DfType::Unknown,
        );
        compares_as(
            vec![Null, "a".into(), "b".into()],
            Dialect::DEFAULT_POSTGRESQL,
            DfType::DEFAULT_TEXT,
        );
        compares_as(
            vec![12u64.into(), (-123).into()],
            Dialect::DEFAULT_MYSQL,
            DfType::BigInt,
        );
        // TODO(ENG-1909)
        // compares_as(vec![12u64.into(), (-123).into()], Dialect::DEFAULT_POSTGRESQL,
        // DfType::Int);
        compares_as(
            vec![12u64.into(), "123".into()],
            Dialect::DEFAULT_MYSQL,
            DfType::DEFAULT_TEXT,
        );
        compares_as(
            vec!["A".into(), "b".into(), 1.into()],
            Dialect::DEFAULT_MYSQL,
            DfType::DEFAULT_TEXT,
        );
    }

    #[test]
    fn eq_returns_bool() {
        let input = parse_expr(ParserDialect::MySQL, "x = 1").unwrap();
        let result = Expr::lower(
            input,
            Dialect::DEFAULT_MYSQL,
            resolve_columns(|c| {
                if c.name == "x" {
                    Ok((0, DfType::DEFAULT_TEXT))
                } else {
                    internal!("what's this column?")
                }
            }),
        )
        .unwrap();
        assert_eq!(*result.ty(), DfType::Bool);
    }

    #[test]
    fn lowered_json_op_expr_types() {
        for op in [
            AstBinaryOperator::QuestionMark,
            AstBinaryOperator::QuestionMarkPipe,
            AstBinaryOperator::QuestionMarkAnd,
        ] {
            let input = AstExpr::BinaryOp {
                lhs: Box::new(AstExpr::Literal("{\"abc\": 42}".into())),
                op,
                rhs: Box::new(AstExpr::Literal("abc".into())),
            };
            let result =
                Expr::lower(input, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
            assert_eq!(*result.ty(), DfType::Bool);
        }
    }

    #[test]
    fn array_expr() {
        let expr = parse_expr(
            ParserDialect::PostgreSQL,
            "ARRAY[[1, '2'::int], array[3, 4]]",
        )
        .unwrap();
        let result = Expr::lower(expr, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
        assert_eq!(
            result,
            Expr::Array {
                elements: vec![
                    Expr::Literal {
                        val: 1u32.into(),
                        ty: DfType::UnsignedBigInt
                    },
                    Expr::Cast {
                        expr: Box::new(Expr::Literal {
                            val: "2".into(),
                            ty: DfType::Unknown
                        }),
                        ty: DfType::Int,
                        null_on_failure: false
                    },
                    Expr::Literal {
                        val: 3u32.into(),
                        ty: DfType::UnsignedBigInt
                    },
                    Expr::Literal {
                        val: 4u32.into(),
                        ty: DfType::UnsignedBigInt
                    }
                ],
                shape: vec![2, 2],
                ty: DfType::Array(Box::new(DfType::Array(Box::new(DfType::UnsignedBigInt))))
            }
        )
    }

    #[test]
    fn op_some_to_any() {
        let expr = parse_expr(ParserDialect::PostgreSQL, "1 = ANY ('{1,2}')").unwrap();
        let result = Expr::lower(expr, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
        assert_eq!(
            result,
            Expr::OpAny {
                op: BinaryOperator::Equal,
                left: Box::new(Expr::Literal {
                    val: 1u64.into(),
                    ty: DfType::UnsignedBigInt
                }),
                right: Box::new(Expr::Cast {
                    expr: Box::new(Expr::Literal {
                        val: "{1,2}".into(),
                        ty: DfType::Unknown
                    }),
                    ty: DfType::Array(Box::new(DfType::UnsignedBigInt)),
                    null_on_failure: false
                }),
                ty: DfType::Bool
            }
        );
    }

    #[test]
    fn op_all() {
        let expr = parse_expr(ParserDialect::PostgreSQL, "1 = ALL ('{1,1}')").unwrap();
        let result = Expr::lower(expr, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
        assert_eq!(
            result,
            Expr::OpAll {
                op: BinaryOperator::Equal,
                left: Box::new(Expr::Literal {
                    val: 1u64.into(),
                    ty: DfType::UnsignedBigInt
                }),
                right: Box::new(Expr::Cast {
                    expr: Box::new(Expr::Literal {
                        val: "{1,1}".into(),
                        ty: DfType::Unknown
                    }),
                    ty: DfType::Array(Box::new(DfType::UnsignedBigInt)),
                    null_on_failure: false
                }),
                ty: DfType::Bool
            }
        );
    }

    #[test]
    fn array_to_string() {
        let expr = parse_expr(
            ParserDialect::PostgreSQL,
            "array_to_string(ARRAY[1], ',', '*')",
        )
        .unwrap();
        let result = Expr::lower(expr, Dialect::DEFAULT_POSTGRESQL, no_op_lower_context()).unwrap();
        assert_eq!(
            result,
            Expr::Call {
                func: Box::new(BuiltinFunction::ArrayToString(
                    Expr::Array {
                        elements: vec![Expr::Literal {
                            val: 1u64.into(),
                            ty: DfType::UnsignedBigInt,
                        }],
                        shape: vec![1],
                        ty: DfType::Array(Box::new(DfType::UnsignedBigInt))
                    },
                    Expr::Literal {
                        val: ",".into(),
                        ty: DfType::Unknown
                    },
                    Some(Expr::Literal {
                        val: "*".into(),
                        ty: DfType::Unknown
                    })
                )),
                ty: DfType::DEFAULT_TEXT
            }
        );
    }
}
