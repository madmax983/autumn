use syn::{Block, Expr, ExprIf, Item, Pat, Stmt};

const REPLAY_GUARD_IDENT: &str = "__AUTUMN_IDEMPOTENCY_REPLAY_GUARD";

pub fn block_has_replay_guard(block: &Block) -> bool {
    let has_marker = block.stmts.iter().any(|stmt| {
        matches!(
            stmt,
            Stmt::Item(Item::Const(item)) if item.ident == REPLAY_GUARD_IDENT
        )
    });

    has_marker && block.stmts.iter().any(stmt_is_generated_replay_guard)
}

fn stmt_is_generated_replay_guard(stmt: &Stmt) -> bool {
    let Stmt::Expr(Expr::If(expr_if), _) = stmt else {
        return false;
    };

    if_let_replays_and_returns(expr_if)
}

fn if_let_replays_and_returns(expr_if: &ExprIf) -> bool {
    let Expr::Let(expr_let) = expr_if.cond.as_ref() else {
        return false;
    };

    pat_is_some_replay_response(&expr_let.pat)
        && expr_is_replay_response_call(&expr_let.expr)
        && block_returns_ident(&expr_if.then_branch, "__autumn_response")
}

fn pat_is_some_replay_response(pat: &Pat) -> bool {
    match pat {
        Pat::TupleStruct(tuple) => {
            path_ends_with(&tuple.path, "Some")
                && tuple.elems.len() == 1
                && pat_binds_ident(&tuple.elems[0], "__autumn_response")
        }
        _ => false,
    }
}

fn pat_binds_ident(pat: &Pat, expected: &str) -> bool {
    matches!(pat, Pat::Ident(ident) if ident.ident == expected)
}

fn expr_is_replay_response_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => path_expr_ends_with(&call.func, "__replay_response"),
        Expr::Group(group) => expr_is_replay_response_call(&group.expr),
        Expr::Paren(paren) => expr_is_replay_response_call(&paren.expr),
        _ => false,
    }
}

fn block_returns_ident(block: &Block, expected: &str) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Stmt::Expr(Expr::Return(ret), _) => ret
            .expr
            .as_ref()
            .is_some_and(|expr| path_expr_ends_with(expr, expected)),
        _ => false,
    })
}

fn path_expr_ends_with(expr: &Expr, expected: &str) -> bool {
    let Expr::Path(path) = expr else {
        return false;
    };

    path_ends_with(&path.path, expected)
}

fn path_ends_with(path: &syn::Path, expected: &str) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == expected)
}

#[cfg(test)]
mod tests {
    use super::block_has_replay_guard;

    #[test]
    fn string_literal_does_not_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            let _ = "__AUTUMN_IDEMPOTENCY_REPLAY_GUARD";
        });

        assert!(!block_has_replay_guard(&block));
    }

    #[test]
    fn marker_const_without_replay_call_does_not_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            let _ = "plain user const";
        });

        assert!(!block_has_replay_guard(&block));
    }

    #[test]
    fn marker_const_and_non_returned_replay_call_do_not_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            let _ignored =
                ::autumn_web::idempotency::__replay_response(&__autumn_idempotency_replay);
        });

        assert!(!block_has_replay_guard(&block));
    }

    #[test]
    fn generated_const_and_replay_call_count_as_replay_guard() {
        let block: syn::Block = syn::parse_quote!({
            const __AUTUMN_IDEMPOTENCY_REPLAY_GUARD: () = ();
            if let ::core::option::Option::Some(__autumn_response) =
                ::autumn_web::idempotency::__replay_response(&__autumn_idempotency_replay)
            {
                return __autumn_response;
            }
        });

        assert!(block_has_replay_guard(&block));
    }
}
