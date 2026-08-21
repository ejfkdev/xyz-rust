// 字段类型形状分析：宏在编译期识别 String/数值/bool/Duration/DateTime/
// Vec<u8>/Vec<T>/Option<T>，其余按透明 XyzField 处理（类型别名、命名
// 标量 newtype、嵌套 XyzArgs struct 都落入此路径——运行时经 trait 分派，
// 无需静态区分）。

use proc_macro2::TokenStream;
use quote::quote;
use syn::{GenericArgument, PathArguments, Type};

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// 标量/别名/newtype/嵌套 struct：经 <T as XyzField> 分派。
    Scalar,
    /// Vec<u8>（Go []byte）。
    Bytes,
    /// Vec<T>（T != u8）。
    Vec(Box<Type>),
    /// Option<T>（Go *T）。
    Opt(Box<Type>),
}

const SCALAR_LAST_IDENTS: &[&str] = &[
    "String", "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
    "Duration", "DateTime",
];

fn last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn shape_of(ty: &Type) -> Shape {
    // 引用字段（&str 等）在接线格式里没有意义——交给 Scalar 路径让 trait
    // 边界报编译错（&str 未 impl XyzField）。
    if let Type::Reference(inner) = ty {
        return shape_of(&inner.elem);
    }
    match ty {
        Type::Path(tp) => {
            let last = tp
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            match last.as_str() {
                "Vec" | "Option" => {
                    let args = match &tp.path.segments.last().unwrap().arguments {
                        PathArguments::AngleBracketed(ab) => &ab.args,
                        _ => return Shape::Scalar,
                    };
                    let first = match args.first() {
                        Some(GenericArgument::Type(t)) => t,
                        _ => return Shape::Scalar,
                    };
                    let elem_ident = last_ident(first).unwrap_or_default();
                    match last.as_str() {
                        "Vec" if elem_ident == "u8" => Shape::Bytes,
                        "Vec" => Shape::Vec(Box::new(first.clone())),
                        _ => Shape::Opt(Box::new(first.clone())),
                    }
                }
                ident if SCALAR_LAST_IDENTS.contains(&ident) => Shape::Scalar,
                _ => Shape::Scalar,
            }
        }
        Type::Group(g) => shape_of(&g.elem),
        Type::Paren(p) => shape_of(&p.elem),
        _ => Shape::Scalar,
    }
}

fn ident(ty: &Type) -> TokenStream {
    quote! { #ty }
}

/// FieldSpec 的基底形状表达式。
pub fn spec_expr(ty: &Type) -> TokenStream {
    match shape_of(ty) {
        Shape::Scalar | Shape::Bytes => {
            let t = ident(ty);
            quote! { ::xyz_rust::spec::spec_of::<#t>() }
        }
        Shape::Vec(inner) => {
            let t = ident(&inner);
            quote! {
                ::xyz_rust::spec::field::synthetic(
                    ::xyz_rust::spec::FieldKind::Slice,
                    Vec::new(),
                    Some(::std::boxed::Box::new(::xyz_rust::spec::spec_of::<#t>())),
                )
            }
        }
        Shape::Opt(inner) => {
            let t = ident(&inner);
            quote! {
                ::xyz_rust::spec::field::synthetic(
                    ::xyz_rust::spec::FieldKind::Ptr,
                    Vec::new(),
                    Some(::std::boxed::Box::new(::xyz_rust::spec::spec_of::<#t>())),
                )
            }
        }
    }
}

/// xyz_decode 里单个字段的解码表达式（meta 索引在 __meta[i]）。
pub fn decode_expr(idx: usize, ty: &Type, skip: bool, map: TokenStream) -> TokenStream {
    let scalar = |t: &Type| {
        let t = ident(t);
        if skip {
            quote! { ::xyz_rust::spec::decode::field_skip::<#t>(&__meta[#idx], #map)? }
        } else {
            quote! { ::xyz_rust::spec::decode::field::<#t>(&__meta[#idx], #map)? }
        }
    };
    match shape_of(ty) {
        Shape::Scalar => scalar(ty),
        Shape::Bytes => {
            let _ = ty;
            if skip {
                quote! { ::xyz_rust::spec::decode::field_skip::<Vec<u8>>(&__meta[#idx], #map)? }
            } else {
                quote! { ::xyz_rust::spec::decode::field_bytes(&__meta[#idx], #map)? }
            }
        }
        Shape::Vec(inner) => {
            let t = ident(&inner);
            if skip {
                quote! { ::xyz_rust::spec::decode::field_skip_vec::<#t>(&__meta[#idx], #map)? }
            } else {
                quote! { ::xyz_rust::spec::decode::field_vec::<#t>(&__meta[#idx], #map)? }
            }
        }
        Shape::Opt(inner) => {
            let t = ident(&inner);
            quote! { ::xyz_rust::spec::decode::field_opt::<#t>(&__meta[#idx], #map)? }
        }
    }
}

/// xyz_validate 里单个字段的校验调用（self_expr = self.字段）。
pub fn validate_expr(idx: usize, ty: &Type, self_expr: TokenStream) -> TokenStream {
    match shape_of(ty) {
        Shape::Scalar => {
            let t = ident(ty);
            quote! {
                ::xyz_rust::spec::validate::check_rules::<#t>(&__meta[#idx], &#self_expr)?;
                <#t as ::xyz_rust::spec::XyzField>::xyz_validate_elem(&#self_expr, &__meta[#idx])?;
            }
        }
        Shape::Bytes => {
            quote! { ::xyz_rust::spec::validate::check_rules::<Vec<u8>>(&__meta[#idx], &#self_expr)?; }
        }
        Shape::Vec(inner) => {
            let t = ident(&inner);
            quote! {
                ::xyz_rust::spec::validate::check_vec_rules::<#t>(&__meta[#idx], &#self_expr)?;
                if let Some(__em) = ::xyz_rust::spec::validate::elem_ref(&__meta[#idx]) {
                    for __it in &#self_expr {
                        <#t as ::xyz_rust::spec::XyzField>::xyz_validate_elem(__it, __em)?;
                    }
                }
            }
        }
        Shape::Opt(inner) => {
            let t = ident(&inner);
            quote! {
                ::xyz_rust::spec::validate::check_opt_rules::<#t>(&__meta[#idx], #self_expr.as_ref())?;
                if let (Some(__it), Some(__em)) = (#self_expr.as_ref(), ::xyz_rust::spec::validate::elem_ref(&__meta[#idx])) {
                    <#t as ::xyz_rust::spec::XyzField>::xyz_validate_elem(__it, __em)?;
                }
            }
        }
    }
}

/// xyz_zero 里单个字段的零值表达式。
pub fn zero_expr(ty: &Type) -> TokenStream {
    match shape_of(ty) {
        Shape::Scalar | Shape::Bytes => {
            let t = ident(ty);
            quote! { <#t as ::xyz_rust::spec::XyzField>::xyz_zero() }
        }
        Shape::Vec(_) => quote! { ::std::vec::Vec::new() },
        Shape::Opt(_) => quote! { ::std::option::Option::None },
    }
}

/// xyz_is_zero 里单个字段的判零项。
pub fn is_zero_term(ty: &Type, self_expr: TokenStream) -> TokenStream {
    match shape_of(ty) {
        Shape::Scalar | Shape::Bytes => {
            let t = ident(ty);
            quote! { <#t as ::xyz_rust::spec::XyzField>::xyz_is_zero(&#self_expr) }
        }
        Shape::Vec(_) => quote! { #self_expr.is_empty() },
        Shape::Opt(_) => quote! { #self_expr.is_none() },
    }
}

/// xyz_type_check 的逐索引校验（fi = &FieldMeta，v = &Value）。
pub fn type_check_expr(ty: &Type, fi: TokenStream, v: TokenStream) -> TokenStream {
    match shape_of(ty) {
        Shape::Scalar | Shape::Bytes => {
            let t = ident(ty);
            quote! { ::xyz_rust::spec::decode::type_check_of::<#t>(#fi, #v) }
        }
        Shape::Vec(inner) => {
            let t = ident(&inner);
            quote! { ::xyz_rust::spec::decode::type_check_vec::<#t>(#fi, #v) }
        }
        Shape::Opt(inner) => {
            let t = ident(&inner);
            quote! { ::xyz_rust::spec::decode::type_check_opt::<#t>(#fi, #v) }
        }
    }
}
