import std._str;
import std._vec;
import std._str.rustrt.sbuf;
import std._vec.rustrt.vbuf;
import std.map.hashmap;

import front.ast;
import driver.session;
import back.x86;
import back.abi;

import util.common.istr;
import util.common.new_str_hash;

import lib.llvm.llvm;
import lib.llvm.builder;
import lib.llvm.llvm.ModuleRef;
import lib.llvm.llvm.ValueRef;
import lib.llvm.llvm.TypeRef;
import lib.llvm.llvm.BuilderRef;
import lib.llvm.llvm.BasicBlockRef;

import lib.llvm.False;
import lib.llvm.True;

type glue_fns = rec(ValueRef activate_glue,
                    ValueRef yield_glue,
                    vec[ValueRef] upcall_glues);

type trans_ctxt = rec(session.session sess,
                      ModuleRef llmod,
                      hashmap[str,ValueRef] upcalls,
                      @glue_fns glues,
                      str path);

type fn_ctxt = rec(ValueRef llfn,
                   ValueRef lloutptr,
                   ValueRef lltaskptr,
                   @trans_ctxt tcx);

type terminator = fn(@fn_ctxt cx, builder build);

type block_ctxt = rec(BasicBlockRef llbb,
                      builder build,
                      terminator term,
                      @fn_ctxt fcx);


// LLVM type constructors.

fn T_nil() -> TypeRef {
    ret llvm.LLVMVoidType();
}

fn T_int() -> TypeRef {
    ret llvm.LLVMInt32Type();
}

fn T_fn(vec[TypeRef] inputs, TypeRef output) -> TypeRef {
    ret llvm.LLVMFunctionType(output,
                              _vec.buf[TypeRef](inputs),
                              _vec.len[TypeRef](inputs),
                              False);
}

fn T_ptr(TypeRef t) -> TypeRef {
    ret llvm.LLVMPointerType(t, 0u);
}

fn T_struct(vec[TypeRef] elts) -> TypeRef {
    ret llvm.LLVMStructType(_vec.buf[TypeRef](elts),
                            _vec.len[TypeRef](elts),
                            False);
}

fn T_opaque() -> TypeRef {
    ret llvm.LLVMOpaqueType();
}

fn T_task() -> TypeRef {
    ret T_struct(vec(T_int(),      // Refcount
                     T_opaque())); // Rest is opaque for now
}


// LLVM constant constructors.

fn C_null(TypeRef t) -> ValueRef {
    ret llvm.LLVMConstNull(t);
}

fn C_int(int i) -> ValueRef {
    // FIXME. We can't use LLVM.ULongLong with our existing minimal native
    // API, which only knows word-sized args.  Lucky for us LLVM has a "take a
    // string encoding" version.  Hilarious. Please fix to handle:
    //
    // ret llvm.LLVMConstInt(T_int(), t as LLVM.ULongLong, False);
    //
    ret llvm.LLVMConstIntOfString(T_int(),
                                  _str.buf(istr(i)), 10);
}

fn C_str(str s) -> ValueRef {
    ret llvm.LLVMConstString(_str.buf(s), _str.byte_len(s), False);
}

fn C_struct(vec[ValueRef] elts) -> ValueRef {
    ret llvm.LLVMConstStruct(_vec.buf[ValueRef](elts),
                             _vec.len[ValueRef](elts),
                             False);
}

fn decl_cdecl_fn(ModuleRef llmod, str name,
                 vec[TypeRef] inputs, TypeRef output) -> ValueRef {
    let TypeRef llty = T_fn(inputs, output);
    log "declaring " + name + " with type " + lib.llvm.type_to_str(llty);
    let ValueRef llfn =
        llvm.LLVMAddFunction(llmod, _str.buf(name), llty);
    llvm.LLVMSetFunctionCallConv(llfn, lib.llvm.LLVMCCallConv);
    ret llfn;
}

fn decl_glue(ModuleRef llmod, str s) -> ValueRef {
    ret decl_cdecl_fn(llmod, s, vec(T_ptr(T_task())), T_nil());
}

fn decl_upcall(ModuleRef llmod, uint _n) -> ValueRef {
    let int n = _n as int;
    let str s = abi.upcall_glue_name(n);
    let vec[TypeRef] args =
        vec(T_ptr(T_task()), // taskptr
            T_int())         // callee
        + _vec.init_elt[TypeRef](T_int(), n as uint);

    ret decl_cdecl_fn(llmod, s, args, T_int());
}

fn get_upcall(@trans_ctxt cx, str name, int n_args) -> ValueRef {
    if (cx.upcalls.contains_key(name)) {
        ret cx.upcalls.get(name);
    }
    auto inputs = vec(T_ptr(T_task()));
    inputs += _vec.init_elt[TypeRef](T_int(), n_args as uint);
    auto output = T_nil();
    auto f = decl_cdecl_fn(cx.llmod, name, inputs, output);
    cx.upcalls.insert(name, f);
    ret f;
}

fn trans_upcall(@block_ctxt cx, str name, vec[ValueRef] args) -> ValueRef {
    let int n = _vec.len[ValueRef](args) as int;
    let ValueRef llupcall = get_upcall(cx.fcx.tcx, name, n);
    llupcall = llvm.LLVMConstPointerCast(llupcall, T_int());

    let ValueRef llglue = cx.fcx.tcx.glues.upcall_glues.(n);
    let vec[ValueRef] call_args = vec(cx.fcx.lltaskptr, llupcall) + args;
    log "emitting indirect-upcall via " + abi.upcall_glue_name(n);
    for (ValueRef v in call_args) {
        log "arg: " + lib.llvm.type_to_str(llvm.LLVMTypeOf(v));
    }
    log "emitting call to callee of type: " +
        lib.llvm.type_to_str(llvm.LLVMTypeOf(llglue));
    ret cx.build.Call(llglue, call_args);
}

fn trans_log(@block_ctxt cx, &ast.atom a) {
    alt (a) {
        case (ast.atom_lit(?lit)) {
            alt (*lit) {
                case (ast.lit_int(?i)) {
                    trans_upcall(cx, "upcall_log_int", vec(C_int(i)));
                }
                case (_) {
                    cx.fcx.tcx.sess.unimpl("literal variant in trans_log");
                }
            }
        }
        case (_) {
            cx.fcx.tcx.sess.unimpl("atom variant in trans_log");
        }
    }
}

fn trans_stmt(@block_ctxt cx, &ast.stmt s) {
    alt (s) {
        case (ast.stmt_log(?a)) {
            trans_log(cx, *a);
        }
        case (_) {
            cx.fcx.tcx.sess.unimpl("stmt variant");
        }
    }
}

fn default_terminate(@fn_ctxt cx, builder build) {
    build.RetVoid();
}

fn trans_block(@fn_ctxt cx, &ast.block b, terminator term) {
    let BasicBlockRef llbb =
        llvm.LLVMAppendBasicBlock(cx.llfn, _str.buf(""));
    let BuilderRef llbuild = llvm.LLVMCreateBuilder();
    llvm.LLVMPositionBuilderAtEnd(llbuild, llbb);
    auto bcx = @rec(llbb=llbb,
                    build=builder(llbuild),
                    term=term,
                    fcx=cx);
    for (@ast.stmt s in b) {
        trans_stmt(bcx, *s);
    }
    bcx.term(cx, bcx.build);
}

fn trans_fn(@trans_ctxt cx, &ast._fn f) {
    let vec[TypeRef] args = vec(T_ptr(T_int()), // outptr.
                                T_ptr(T_task()) // taskptr
                                );
    let ValueRef llfn = decl_cdecl_fn(cx.llmod, cx.path, args, T_nil());
    let ValueRef lloutptr = llvm.LLVMGetParam(llfn, 0u);
    let ValueRef lltaskptr = llvm.LLVMGetParam(llfn, 1u);
    auto fcx = @rec(llfn=llfn,
                    lloutptr=lloutptr,
                    lltaskptr=lltaskptr,
                    tcx=cx);
    auto term = default_terminate;
    trans_block(fcx, f.body, term);
}

fn trans_item(@trans_ctxt cx, &str name, &ast.item item) {
    auto sub_cx = @rec(path=cx.path + "." + name with *cx);
    alt (item) {
        case (ast.item_fn(?f)) {
            trans_fn(sub_cx, *f);
        }
        case (ast.item_mod(?m)) {
            trans_mod(sub_cx, *m);
        }
    }
}

fn trans_mod(@trans_ctxt cx, &ast._mod m) {
    for each (tup(str, ast.item) pair in m.items()) {
        trans_item(cx, pair._0, pair._1);
    }
}

fn trans_crate(session.session sess, ast.crate crate) {
    auto llmod =
        llvm.LLVMModuleCreateWithNameInContext(_str.buf("rust_out"),
                                               llvm.LLVMGetGlobalContext());

    llvm.LLVMSetModuleInlineAsm(llmod, _str.buf(x86.get_module_asm()));

    auto glues = @rec(activate_glue = decl_glue(llmod,
                                                abi.activate_glue_name()),
                      yield_glue = decl_glue(llmod, abi.yield_glue_name()),
                      upcall_glues =
                      _vec.init_fn[ValueRef](bind decl_upcall(llmod, _),
                                             abi.n_upcall_glues as uint));

    auto cx = @rec(sess = sess,
                   llmod = llmod,
                   upcalls = new_str_hash[ValueRef](),
                   glues = glues,
                   path = "");

    trans_mod(cx, crate.module);

    llvm.LLVMWriteBitcodeToFile(llmod, _str.buf("rust_out.bc"));
    llvm.LLVMDisposeModule(llmod);
}

//
// Local Variables:
// mode: rust
// fill-column: 78;
// indent-tabs-mode: nil
// c-basic-offset: 4
// buffer-file-coding-system: utf-8-unix
// compile-command: "make -k -C ../.. 2>&1 | sed -e 's/\\/x\\//x:\\//g'";
// End:
//
