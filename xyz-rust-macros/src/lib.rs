//! xyz-rust 的派生宏：XyzArgs（入参 struct）、XyzField（命名标量
//! newtype）、XyzOutput（结果 struct 的 JSON Schema）。
//!
//! 全部生成代码经 `::xyz_rust::...` 绝对路径引用运行时（用户 crate 名
//! 无关，只要 xyz-rust 在依赖图里）。

mod rename;
mod shape;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields};

/// 字段/结构上的 #[xyz(...)] 属性解析结果。
#[derive(Default, Clone)]
struct XyzAttr {
    desc: Option<String>,
    name: Option<String>,
    required: bool,
    secret: bool,
    skip: bool,
    default: Option<String>,
    enum_s: Option<String>,
    validate: Option<String>,
    cli: Option<String>,
    http: Option<String>,
    http_name: Option<String>,
}

fn parse_xyz_attr(attrs: &[syn::Attribute]) -> syn::Result<XyzAttr> {
    let mut out = XyzAttr::default();
    for attr in attrs {
        if !attr.path().is_ident("xyz") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("required") {
                out.required = true;
                Ok(())
            } else if meta.path.is_ident("secret") {
                out.secret = true;
                Ok(())
            } else if meta.path.is_ident("skip") {
                out.skip = true;
                Ok(())
            } else if meta.path.is_ident("desc") {
                out.desc = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("name") {
                out.name = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("default") {
                out.default = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("enum") {
                out.enum_s = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("validate") {
                out.validate = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("cli") {
                out.cli = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("http") {
                out.http = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("http_name") {
                out.http_name = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error(
                    "unknown #[xyz] option (want desc|name|required|secret|skip|default|enum|validate|cli|http|http_name)",
                ))
            }
        })?;
    }
    Ok(out)
}

/// 极简 serde 属性子集：rename / rename_all / skip。
fn serde_pairs(attrs: &[syn::Attribute]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let Ok(list) = attr.meta.require_list() else {
            continue;
        };
        let body = list.tokens.to_string();
        let mut rest = body.trim();
        while let Some(eq) = rest.find('=') {
            let key = rest[..eq].trim().trim_start_matches(',').trim().to_string();
            let after = rest[eq + 1..].trim_start();
            if let Some(after_str) = after.strip_prefix('"')
                && let Some(val) = after_str.split('"').next()
            {
                out.push((key.clone(), val.to_string()));
                // 跳过该字符串值
                let consumed = after_str.find('"').map(|p| p + 1).unwrap_or(0);
                rest = &after[consumed..];
                continue;
            }
            // 无等号值的 flag（skip）或无法解析：吃掉一个 token
            let skip_to = after
                .find(|c: char| c == ',' && !after[..after.find(c).unwrap_or(0)].contains('"'))
                .map(|p| p + 1)
                .unwrap_or(after.len());
            rest = &after[skip_to.min(after.len())..];
        }
    }
    out
}

fn serde_rename(attrs: &[syn::Attribute]) -> Option<String> {
    serde_pairs(attrs)
        .into_iter()
        .find(|(k, _)| k == "rename")
        .map(|(_, v)| v)
}

fn serde_rename_all(attrs: &[syn::Attribute]) -> Option<String> {
    serde_pairs(attrs)
        .into_iter()
        .find(|(k, _)| k == "rename_all")
        .map(|(_, v)| v)
}

/// serde `#[serde(tag = "…")]` 的判别键（联合枚举必需，spec §4.7）。
fn serde_tag(attrs: &[syn::Attribute]) -> Option<String> {
    serde_pairs(attrs)
        .into_iter()
        .find(|(k, _)| k == "tag")
        .map(|(_, v)| v)
}

fn serde_skipped(attrs: &[syn::Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let Ok(list) = attr.meta.require_list() else {
            continue;
        };
        let body = list.tokens.to_string();
        if body.trim() == "skip" {
            return true;
        }
        // serde(skip) / serde(skip_serializing) / serde(skip_deserializing)
        for tok in body.split(',') {
            let t = tok.trim();
            if t == "skip" || t == "skip_serializing" || t == "skip_deserializing" {
                return true;
            }
        }
    }
    false
}

/// 字段描述：rust 名 + 线上名 + 属性。
struct FlexField {
    rust_name: String,
    json_name: String,
    attrs: XyzAttr,
    ty: syn::Type,
}

fn wire_name(
    rust: &str,
    attrs: &XyzAttr,
    serde_rename: Option<&str>,
    rename_all: Option<&str>,
) -> String {
    if let Some(n) = &attrs.name {
        return n.clone();
    }
    if let Some(r) = serde_rename {
        return r.to_string();
    }
    if let Some(style) = rename_all {
        return rename::apply(style, rust);
    }
    rust.to_string()
}

/// 收集具名字段并做编译期校验。
fn collect_fields(ident: &syn::Ident, data: &Data) -> syn::Result<Vec<FlexField>> {
    let fields = match data {
        Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "#[derive(XyzArgs)] 只支持具名字段的 struct",
            ));
        }
    };
    let rename_all = serde_rename_all(
        &fields
            .iter()
            .next()
            .map(|f| f.attrs.clone())
            .unwrap_or_default(),
    );
    let mut out = Vec::new();
    let mut seen_self = false;
    for f in fields {
        let (Some(fident), ty) = (&f.ident, &f.ty) else {
            return Err(syn::Error::new_spanned(
                f,
                "#[derive(XyzArgs)] 不支持元组 struct（字段必须有名字）",
            ));
        };
        let attrs = parse_xyz_attr(&f.attrs)?;
        if attrs.skip && attrs.required {
            return Err(syn::Error::new_spanned(
                f,
                "required 与 skip 冲突（skip ≡ json:\"-\"）",
            ));
        }
        // 静态自引用护栏：字段类型（含嵌套容器）引用自身将无限递归。
        let ty_str = quote!(#ty).to_string();
        if ty_str
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|tok| ident == tok)
        {
            seen_self = true;
        }
        let serde_r = serde_rename(&f.attrs);
        let json_name = wire_name(
            &fident.to_string(),
            &attrs,
            serde_r.as_deref(),
            rename_all.as_deref(),
        );
        out.push(FlexField {
            rust_name: fident.to_string(),
            json_name,
            attrs,
            ty: ty.clone(),
        });
    }
    if seen_self {
        return Err(syn::Error::new_spanned(
            ident,
            "字段类型引用了自身：递归类型无法映射到接线格式",
        ));
    }
    Ok(out)
}

fn struct_desc(attrs: &[syn::Attribute]) -> Option<String> {
    parse_xyz_attr(attrs).ok().and_then(|a| a.desc)
}

// ---------------------------------------------------------------------------
// #[derive(XyzArgs)]
// ---------------------------------------------------------------------------

#[proc_macro_derive(XyzArgs, attributes(xyz, serde))]
pub fn derive_xyz_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_xyz_args(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn expand_xyz_args(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    // spec §4.7：enum = 邻接带标签联合。作为字段类型参与三通道
    // （XyzField）；不可直接当 define 的根参数（根仍是 struct）。
    if let Data::Enum(e) = &input.data {
        return expand_enum_args(&input.ident, &input.attrs, e);
    }
    let name = &input.ident;
    let flex = collect_fields(name, &input.data)?;
    let n_fields = flex.len();

    let spec_exprs = flex.iter().map(|f| {
        let lit = |s: String| syn::LitStr::new(&s, proc_macro2::Span::call_site());
        let rust_name = lit(f.rust_name.clone());
        let json_name = lit(f.json_name.clone());
        let a = &f.attrs;
        let desc = lit(a.desc.clone().unwrap_or_default());
        let required = a.required;
        let secret = a.secret;
        let skip = a.skip;
        let validate = lit(a.validate.clone().unwrap_or_default());
        let enum_s = lit(a.enum_s.clone().unwrap_or_default());
        let default_s = lit(a.default.clone().unwrap_or_default());
        let cli_s = lit(a.cli.clone().unwrap_or_default());
        let http_s = lit(a.http.clone().unwrap_or_default());
        let http_name = match &a.http_name {
            Some(n) => {
                let l = syn::LitStr::new(n, proc_macro2::Span::call_site());
                quote! { Some(#l) }
            }
            None => quote! { None },
        };
        let shape = shape::spec_expr(&f.ty);
        quote! {{
            let mut __s = #shape;
            __s.rust_name = #rust_name;
            __s.json_name = #json_name;
            __s.desc = #desc;
            __s.required = #required;
            __s.secret = #secret;
            __s.skip = #skip;
            __s.validate_s = #validate;
            __s.enum_s = #enum_s;
            __s.default_s = #default_s;
            __s.cli_s = #cli_s;
            __s.http_s = #http_s;
            __s.http_name = #http_name;
            __s
        }}
    });

    let decode_fields = flex.iter().enumerate().map(|(i, f)| {
        let fident = format_ident!("{}", f.rust_name);
        let dec = shape::decode_expr(i, &f.ty, f.attrs.skip, quote! { __map });
        quote! { #fident: #dec, }
    });

    let validate_calls = flex.iter().enumerate().map(|(i, f)| {
        let fident = format_ident!("{}", f.rust_name);
        shape::validate_expr(i, &f.ty, quote! { self.#fident })
    });

    let zero_fields = flex.iter().map(|f| {
        let fident = format_ident!("{}", f.rust_name);
        let z = shape::zero_expr(&f.ty);
        quote! { #fident: #z, }
    });

    let is_zero_terms = flex.iter().map(|f| {
        let fident = format_ident!("{}", f.rust_name);
        shape::is_zero_term(&f.ty, quote! { self.#fident })
    });

    let type_check_arms = flex.iter().enumerate().map(|(i, f)| {
        let check = shape::type_check_expr(&f.ty, quote! { __fi }, quote! { __v });
        quote! { #i => { let __fi = &__meta[#i]; #check } }
    });

    Ok(quote! {
        impl ::xyz_rust::spec::XyzArgs for #name {
            fn xyz_spec() -> Vec<::xyz_rust::spec::FieldSpec> {
                ::xyz_rust::spec::field::spec_depth_guard();
                vec![ #(#spec_exprs),* ]
            }

            fn xyz_meta() -> ::xyz_rust::errors::Result<&'static [::xyz_rust::spec::FieldMeta]> {
                static __CACHE: ::std::sync::OnceLock<
                    ::xyz_rust::errors::Result<Vec<::xyz_rust::spec::FieldMeta>>,
                > = ::std::sync::OnceLock::new();
                let __r = __CACHE.get_or_init(|| {
                    let mut __out = Vec::new();
                    for __spec in <#name as ::xyz_rust::spec::XyzArgs>::xyz_spec().iter() {
                        match ::xyz_rust::spec::field::meta_from_spec(__spec) {
                            Ok(__m) => __out.push(__m),
                            Err(__e) => {
                                return Err(::xyz_rust::errors::Error::new(__e.kind(), __e.to_string()));
                            }
                        }
                    }
                    Ok(__out)
                });
                match __r {
                    Ok(__v) => Ok(__v.as_slice()),
                    Err(__e) => Err(::xyz_rust::errors::Error::new(__e.kind(), __e.to_string())),
                }
            }

            fn xyz_decode(
                __map: &::xyz_rust::spec::JsonMap,
                __meta: &[::xyz_rust::spec::FieldMeta],
            ) -> ::xyz_rust::errors::Result<Self> {
                ::xyz_rust::spec::decode::expect_meta_len(__meta, #n_fields)?;
                Ok(Self { #(#decode_fields)* })
            }

            fn xyz_validate(
                &self,
                __meta: &[::xyz_rust::spec::FieldMeta],
            ) -> ::xyz_rust::errors::Result<()> {
                #(#validate_calls)*
                Ok(())
            }

            fn xyz_type_check(
                __idx: usize,
                __meta: &[::xyz_rust::spec::FieldMeta],
                __v: &::xyz_rust::serde_json::Value,
            ) -> ::xyz_rust::errors::Result<()> {
                match __idx {
                    #(#type_check_arms,)*
                    _ => Err(::xyz_rust::errors::Error::new(
                        ::xyz_rust::errors::Kind::Internal,
                        "internal: field index out of range".to_string(),
                    )),
                }
            }
        }

        impl ::xyz_rust::spec::XyzField for #name {
            fn xyz_from_value(
                __v: &::xyz_rust::serde_json::Value,
            ) -> ::xyz_rust::errors::Result<Self> {
                let (__m, __g) = ::xyz_rust::spec::decode::object_arg(__v)?;
                let __meta = <#name as ::xyz_rust::spec::XyzArgs>::xyz_meta()?;
                let __s = <#name as ::xyz_rust::spec::XyzArgs>::xyz_decode(__m, __meta)?;
                <#name as ::xyz_rust::spec::XyzArgs>::xyz_validate(&__s, __meta)?;
                drop(__g);
                Ok(__s)
            }
            fn xyz_zero() -> Self {
                Self { #(#zero_fields)* }
            }
            fn xyz_is_zero(&self) -> bool {
                true #( && #is_zero_terms )*
            }
            fn xyz_rule_ok(&self, __r: &::xyz_rust::spec::validate::VRule) -> bool {
                // struct 无数值/长度语义（Go numericOf 的兜底不值得复刻）。
                let _ = __r;
                false
            }
            fn xyz_fmt(&self) -> String {
                ::std::any::type_name::<Self>().to_string()
            }
            fn xyz_spec_of() -> ::xyz_rust::spec::FieldSpec {
                ::xyz_rust::spec::field::spec_depth_guard();
                ::xyz_rust::spec::field::synthetic(
                    ::xyz_rust::spec::FieldKind::Struct,
                    <#name as ::xyz_rust::spec::XyzArgs>::xyz_spec(),
                    None,
                )
            }
            fn xyz_validate_elem(
                &self,
                __meta: &::xyz_rust::spec::FieldMeta,
            ) -> ::xyz_rust::errors::Result<()> {
                <#name as ::xyz_rust::spec::XyzArgs>::xyz_validate(self, &__meta.children)
            }
        }

        impl ::xyz_rust::spec::XyzSchema for #name {
            fn xyz_schema() -> Option<::xyz_rust::spec::Schema> {
                <#name as ::xyz_rust::spec::XyzArgs>::xyz_meta()
                    .ok()
                    .map(|__fields| ::xyz_rust::spec::schema::build_schema(__fields))
            }
        }
    })
}

// ---------------------------------------------------------------------------
/// spec §4.7：enum 派生器——生成 XyzField（联合字段）。要求
/// `#[serde(tag = "…")]` 邻接标签；变体须具名（struct 式）字段。
/// enum 作 define 根参数暂不开放（根仍是 struct）。
fn expand_enum_args(
    ident: &syn::Ident,
    attrs: &[syn::Attribute],
    data: &syn::DataEnum,
) -> syn::Result<proc_macro2::TokenStream> {
    let tag = serde_tag(attrs).ok_or_else(|| {
        syn::Error::new_spanned(
            ident,
            "xyz: union enums require #[serde(tag = \"…\")] adjacent tagging (spec §4.7)",
        )
    })?;
    let mut variant_tokens = Vec::new();
    for v in &data.variants {
        let vname = v.ident.to_string();
        let mut field_specs = Vec::new();
        for f in &v.fields {
            let fident = match &f.ident {
                Some(i) => i.to_string(),
                None => {
                    return Err(syn::Error::new_spanned(
                        f,
                        "xyz: tuple union variants are not supported; use named fields",
                    ));
                }
            };
            let attrs = parse_xyz_attr(&f.attrs)?;
            if attrs.skip && attrs.required {
                return Err(syn::Error::new_spanned(f, "required 与 skip 冲突"));
            }
            let json_name = attrs
                .name
                .clone()
                .or_else(|| serde_rename(&f.attrs))
                .unwrap_or_else(|| fident.clone());
            let flex = FlexField {
                rust_name: fident,
                json_name,
                attrs,
                ty: f.ty.clone(),
            };
            field_specs.push(field_spec_literal(&flex));
        }
        let vlit = syn::LitStr::new(&vname, proc_macro2::Span::call_site());
        variant_tokens.push(quote! {
            ::xyz_rust::spec::field::UnionVariantSpec {
                name: #vlit,
                fields: vec![ #(#field_specs),* ],
            }
        });
    }
    Ok(quote! {
        impl ::xyz_rust::spec::XyzField for #ident {
            fn xyz_spec_of() -> ::xyz_rust::spec::field::FieldSpec {
                let mut __s = ::xyz_rust::spec::field::synthetic(
                    ::xyz_rust::spec::field::FieldKind::Union,
                    Vec::new(),
                    None,
                );
                __s.union = Some(::xyz_rust::spec::field::UnionSpec {
                    tag: #tag,
                    variants: vec![ #(#variant_tokens),* ],
                });
                __s
            }
            fn xyz_from_value(
                __v: &::xyz_rust::serde_json::Value,
            ) -> ::xyz_rust::errors::Result<Self> {
                ::xyz_rust::serde_json::from_value(__v.clone()).map_err(|e| {
                    ::xyz_rust::errors::Error::new(
                        ::xyz_rust::errors::Kind::InvalidInput,
                        format!("union decode: {e}"),
                    )
                })
            }
            fn xyz_zero() -> Self {
                unreachable!("unions have no zero value (xyz_is_zero is always false)")
            }
            fn xyz_is_zero(&self) -> bool {
                false
            }
            fn xyz_rule_ok(&self, _r: &::xyz_rust::spec::validate::VRule) -> bool {
                false
            }
            fn xyz_fmt(&self) -> String {
                "<union>".to_string()
            }
        }
    })
}

/// 单个变体字段的 FieldSpec 字面量（与 struct 主路径同构）。
fn field_spec_literal(f: &FlexField) -> proc_macro2::TokenStream {
    let lit = |s: &str| syn::LitStr::new(s, proc_macro2::Span::call_site());
    let rust_name = lit(&f.rust_name);
    let json_name = lit(&f.json_name);
    let a = &f.attrs;
    let desc = lit(a.desc.as_deref().unwrap_or_default());
    let required = a.required;
    let secret = a.secret;
    let skip = a.skip;
    let validate = lit(a.validate.as_deref().unwrap_or_default());
    let enum_s = lit(a.enum_s.as_deref().unwrap_or_default());
    let default_s = lit(a.default.as_deref().unwrap_or_default());
    let cli_s = lit(a.cli.as_deref().unwrap_or_default());
    let http_s = lit(a.http.as_deref().unwrap_or_default());
    let http_name = match &a.http_name {
        Some(n) => {
            let l = syn::LitStr::new(n, proc_macro2::Span::call_site());
            quote! { Some(#l) }
        }
        None => quote! { None },
    };
    let shape = shape::spec_expr(&f.ty);
    quote! {{
        let mut __s = #shape;
        __s.rust_name = #rust_name;
        __s.json_name = #json_name;
        __s.desc = #desc;
        __s.required = #required;
        __s.secret = #secret;
        __s.skip = #skip;
        __s.validate_s = #validate;
        __s.enum_s = #enum_s;
        __s.default_s = #default_s;
        __s.cli_s = #cli_s;
        __s.http_s = #http_s;
        __s.http_name = #http_name;
        __s
    }}
}

// #[derive(XyzField)] — 命名标量 newtype：单字段元组 struct
// ---------------------------------------------------------------------------

#[proc_macro_derive(XyzField, attributes(xyz, serde))]
pub fn derive_xyz_field(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_xyz_field(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn expand_xyz_field(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let inner_ty = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Unnamed(u) if u.unnamed.len() == 1 => u.unnamed[0].ty.clone(),
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(XyzField)] 只支持单字段元组 struct（如 struct Port(i32);）",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(XyzField)] 只支持单字段元组 struct",
            ));
        }
    };
    Ok(quote! {
        impl ::xyz_rust::spec::XyzField for #name {
            fn xyz_from_value(
                __v: &::xyz_rust::serde_json::Value,
            ) -> ::xyz_rust::errors::Result<Self> {
                Ok(#name(<#inner_ty as ::xyz_rust::spec::XyzField>::xyz_from_value(__v)?))
            }
            fn xyz_zero() -> Self {
                #name(<#inner_ty as ::xyz_rust::spec::XyzField>::xyz_zero())
            }
            fn xyz_is_zero(&self) -> bool {
                <#inner_ty as ::xyz_rust::spec::XyzField>::xyz_is_zero(&self.0)
            }
            fn xyz_rule_ok(&self, __r: &::xyz_rust::spec::validate::VRule) -> bool {
                <#inner_ty as ::xyz_rust::spec::XyzField>::xyz_rule_ok(&self.0, __r)
            }
            fn xyz_fmt(&self) -> String {
                <#inner_ty as ::xyz_rust::spec::XyzField>::xyz_fmt(&self.0)
            }
            fn xyz_spec_of() -> ::xyz_rust::spec::FieldSpec {
                <#inner_ty as ::xyz_rust::spec::XyzField>::xyz_spec_of()
            }
            fn xyz_validate_elem(
                &self,
                __meta: &::xyz_rust::spec::FieldMeta,
            ) -> ::xyz_rust::errors::Result<()> {
                <#inner_ty as ::xyz_rust::spec::XyzField>::xyz_validate_elem(&self.0, __meta)
            }
        }
        impl ::xyz_rust::spec::XyzSchema for #name {
            fn xyz_schema() -> Option<::xyz_rust::spec::Schema> {
                <#inner_ty as ::xyz_rust::spec::XyzSchema>::xyz_schema()
            }
        }
    })
}

// ---------------------------------------------------------------------------
// #[derive(XyzOutput)] — 结果 struct 的 JSON Schema（serde 感知）
// ---------------------------------------------------------------------------

#[proc_macro_derive(XyzOutput, attributes(xyz, serde))]
pub fn derive_xyz_output(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_xyz_output(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn expand_xyz_output(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(XyzOutput)] 只支持具名字段的 struct",
            ));
        }
    };
    let rename_all = serde_rename_all(
        &fields
            .iter()
            .next()
            .map(|f| f.attrs.clone())
            .unwrap_or_default(),
    );
    let mut prop_entries = Vec::new();
    let mut required_names = Vec::new();
    for f in fields {
        let Some(fident) = &f.ident else {
            return Err(syn::Error::new_spanned(f, "元组字段不支持"));
        };
        if serde_skipped(&f.attrs) {
            continue;
        }
        let attrs = parse_xyz_attr(&f.attrs)?;
        if attrs.required {
            required_names.push(syn::LitStr::new(
                &wire_name(
                    &fident.to_string(),
                    &attrs,
                    serde_rename(&f.attrs).as_deref(),
                    rename_all.as_deref(),
                ),
                proc_macro2::Span::call_site(),
            ));
        }
        let wire = wire_name(
            &fident.to_string(),
            &attrs,
            serde_rename(&f.attrs).as_deref(),
            rename_all.as_deref(),
        );
        let json_name = syn::LitStr::new(&wire, proc_macro2::Span::call_site());
        let ty = &f.ty;
        prop_entries.push(quote! {
            if let Some(__s) = <#ty as ::xyz_rust::spec::XyzSchema>::xyz_schema() {
                let __v = ::xyz_rust::spec::schema::schema_to_value(&__s);
                __props.insert(#json_name.to_string(), __v);
            }
        });
    }
    let struct_desc = syn::LitStr::new(
        &struct_desc(&input.attrs).unwrap_or_default(),
        proc_macro2::Span::call_site(),
    );
    let required_ctor = if required_names.is_empty() {
        quote! { None }
    } else {
        quote! { Some(vec![ #(#required_names.to_string()),* ]) }
    };
    Ok(quote! {
        impl ::xyz_rust::spec::XyzSchema for #name {
            fn xyz_schema() -> Option<::xyz_rust::spec::Schema> {
                let mut __props = ::xyz_rust::serde_json::Map::new();
                #(#prop_entries)*
                Some(::xyz_rust::spec::Schema {
                    r#type: Some("object".to_string()),
                    description: if #struct_desc.is_empty() {
                        None
                    } else {
                        Some(#struct_desc.to_string())
                    },
                    properties: Some(__props),
                    required: #required_ctor,
                    items: None,
                    r#enum: None,
                    default: None,
                    format: None,
                    one_of: None,
                    r#const: None,
                })
            }
        }
    })
}
