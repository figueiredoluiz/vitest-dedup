use super::*;

/// Deferred methods are not executed events and repeated construction stays compact.
#[test]
fn owned_class_summaries_are_compact_and_phase_separated() {
    let inert = behavior_of("class C { method() { tick(); } } expect(1).toBe(1);");
    assert_eq!(inert.len(), 1, "{inert:?}");
    let initialized = behavior_of(
        "class C { static { tick(); } method() { deferred(); } } new C(); expect(1).toBe(1);",
    );
    assert_eq!(initialized.len(), 3, "{initialized:?}");
    assert!(initialized[0].starts_with("class-eval:"), "{initialized:?}");
    assert!(initialized[1].starts_with("new:"), "{initialized:?}");
    let methods = (0..100)
        .map(|index| format!("method{index}() {{ return {index}; }}"))
        .collect::<String>();
    let events = behavior_of(&format!(
        "class C {{ {methods} }} {}",
        "new C();".repeat(20)
    ));
    assert_eq!(events.len(), 20);
    assert!(
        events.iter().all(|event| event.len() < 512),
        "class body must not expand into each event"
    );
}

/// Unsupported state is visible and scoped to its consumers, preserving unrelated matches.
#[test]
fn owned_class_uncertainty_is_visible_and_body_scoped() {
    use crate::model::Rule;
    for body in [
        "const trace=[]; const alias=trace; class C{constructor(){trace.push(1)}} alias.push(99); new C(); expect(trace).toEqual([1]);",
        "let x=1; if(flag) x=2; class C{value=x} new C(); expect(1).toBe(1);",
        "class C{value=1} const Alias=C; Alias.value=2; new C(); expect(1).toBe(1);",
        "class C{value=1} C.prototype.value=2; new C(); expect(1).toBe(1);",
        "class C{value=1} expect(C.toString()).toBe('source'); new C();",

    ] {
        let source = format!(
            "import {{test,expect}} from 'vitest'; test('a',()=>{{class C{{value=1}} new C(); expect(1).toBe(1)}}); test('b',()=>{{class C{{value=1}} new C(); expect(1).toBe(1)}}); test('uncertain',()=>{{{body}}});"
        );
        let findings = class_outcome_findings(&source, 3, 1);
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == Rule::DuplicateTest)
                .count(),
            1,
            "{source}: {findings:?}"
        );
        assert_eq!(
            findings.len(),
            1,
            "only the known pair is comparable: {findings:?}"
        );
    }
}

/// Ordinary instance names are modeled properties, rather than class-name reflection.
#[test]
fn owned_class_review_instance_named_properties() {
    let source = "import {test,expect} from 'vitest'; test('a',()=>{class User{name='a'; toString=1; prototype=2} const u=new User(); expect(u.name).toBe('a'); expect(u.toString).toBe(1); expect(u.prototype).toBe(2)}); test('b',()=>{class User{name='a'; toString=1; prototype=2} const u=new User(); expect(u.name).toBe('a'); expect(u.toString).toBe(1); expect(u.prototype).toBe(2)});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// An unrelated built-in construction cannot invalidate a private owned capture.
#[test]
fn owned_class_review_unrelated_constructor() {
    let source = "import {test,expect} from 'vitest'; test('a',()=>{const calls=[]; const map=new Map(); class C{constructor(){calls.push(1)}} new C(); expect(calls).toEqual([1])}); test('b',()=>{const calls=[]; const map=new Map(); class C{constructor(){calls.push(1)}} new C(); expect(calls).toEqual([1])});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// Exact runtime source passes one test and fails one when an inferred callable name differs.
#[test]
fn owned_class_callable_name_runtime_counterexample() {
    use crate::model::Rule;
    let source="import {test,expect} from 'vitest';\ntest('first',()=>{const first=()=>1;class C{field=first.name}const c=new C();expect(c.field).toBe('first')});\ntest('second',()=>{const second=()=>1;class C{field=second.name}const c=new C();expect(c.field).toBe('first')});\n";
    for source in [
        source.to_owned(),
        source
            .replace(
                "class C{field=first.name}",
                "const key='name';class C{field=first[key]}",
            )
            .replace(
                "class C{field=second.name}",
                "const key='name';class C{field=second[key]}",
            ),
        source
            .replace(
                "class C{field=first.name}",
                "const key='name';const alias=key;class C{field=first[alias]}",
            )
            .replace(
                "class C{field=second.name}",
                "const key='name';const alias=key;class C{field=second[alias]}",
            ),
    ] {
        let findings = complete_pair_findings(&source);
        assert!(
            !findings.iter().any(|finding| matches!(
                finding.rule,
                Rule::DuplicateTest | Rule::NearDuplicateTest
            )),
            "{source}: {findings:?}"
        );
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
                .count(),
            1,
            "{source}"
        );
    }
}

/// Observed private callable names preserve runtime values without blocking ordinary renaming.
#[test]
fn owned_class_private_callable_reflection() {
    use crate::model::Rule;
    let observed="import {test,expect} from 'vitest';\ntest('first',()=>{class C{#first(){return 1}read(){return this.#first.name}}const c=new C();expect(c.read()).toBe('#first')});\ntest('second',()=>{class C{#second(){return 1}read(){return this.#second.name}}const c=new C();expect(c.read()).toBe('#first')});\n";
    let findings = complete_pair_findings(observed);
    assert_class_distinct(&findings, observed);
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
            .count(),
        1
    );
    for (first,second,duplicate) in [
        ("class C{#first(){return 1}read(){return this.#first()}}const c=new C();expect(c.read()).toBe(1)","class C{#second(){return 1}read(){return this.#second()}}const c=new C();expect(c.read()).toBe(1)",true),
        ("class C{#first(){return 1}read(){return this.#first.call(this)}}const c=new C();expect(c.read()).toBe(1)","class C{#second(){return 1}read(){return this.#second.call(this)}}const c=new C();expect(c.read()).toBe(1)",true),
        ("class C{#first(){return 1}read(){const alias=this.#first;return alias.name}}const c=new C();expect(c.read()).toBe('#first')","class C{#second(){return 1}read(){const alias=this.#second;return alias.name}}const c=new C();expect(c.read()).toBe('#first')",false),
        ("class C{#first=()=>1;read(){return this.#first()}}const c=new C();expect(c.read()).toBe(1)","class C{#second=()=>1;read(){return this.#second()}}const c=new C();expect(c.read()).toBe(1)",true),
        ("class C{#first=function stable(){return 1};read(){return this.#first.name}}const c=new C();expect(c.read()).toBe('stable')","class C{#second=function stable(){return 1};read(){return this.#second.name}}const c=new C();expect(c.read()).toBe('stable')",true),
        ("class C{#first=()=>1;read(){return this.#first.name}}const c=new C();expect(c.read()).toBe('#first')","class C{#second=()=>1;read(){return this.#second.name}}const c=new C();expect(c.read()).toBe('#first')",false),
        ("class C{static #first(){return 1}static read(){const key='name';return C.#first[key]}}expect(C.read()).toBe('#first')","class C{static #second(){return 1}static read(){const key='name';return C.#second[key]}}expect(C.read()).toBe('#first')",false),
        ("class C{#state={name:'ready',toString:'shown'};read(){return this.#state.name+this.#state.toString}}const c=new C();expect(c.read()).toBe('readyshown')","class C{#renamed={name:'ready',toString:'shown'};read(){return this.#renamed.name+this.#renamed.toString}}const c=new C();expect(c.read()).toBe('readyshown')",true),
        ("const state={name:'ready'};class C{#state=state;read(){return this.#state.name}}const c=new C();expect(c.read()).toBe('ready')","const state={name:'ready'};class C{#renamed=state;read(){return this.#renamed.name}}const c=new C();expect(c.read()).toBe('ready')",true),
        ("class C{#state={name:'ready'};read(){const alias=this.#state;return alias.name}}const c=new C();expect(c.read()).toBe('ready')","class C{#renamed={name:'ready'};read(){const alias=this.#renamed;return alias.name}}const c=new C();expect(c.read()).toBe('ready')",true),
    ] {
        let source=format!("import {{test,expect}} from 'vitest';test('first',()=>{{{first}}});test('second',()=>{{{second}}});");
        let findings=complete_pair_findings(&source);
        if duplicate { assert_eq!(findings.iter().filter(|finding|finding.rule==Rule::DuplicateTest).count(),1,"{source}: {findings:?}"); }
        else {assert_class_distinct(&findings,&source);}
    }
    for body in [
        "class C{#first(){return 1}read(){return this.#first.toString()}}const c=new C();expect(c.read()).toBe('source')",
        "class C{#first(){return 1}read(){return this.#first}}const c=new C();expect(c.read().name).toBe('#first')",
        "class C{#first=()=>1;read(){const key='toString';return this.#first[key]()}}const c=new C();expect(c.read()).toBe('source')",
        "class C{#first=()=>1;constructor(){this.#first=function replacement(){return 1}}read(){return this.#first.name}}const c=new C();expect(c.read()).toBe('replacement')",
        "class C{#first=()=>1;read(){Object.defineProperty(this.#first,'name',{value:'changed'});return this.#first.name}}const c=new C();expect(c.read()).toBe('changed')",
        "class C{#first=()=>1;read(){const alias=this.#first;Object.defineProperty(alias,'name',{value:'changed'});return this.#first.name}}const c=new C();expect(c.read()).toBe('changed')",
        "class C{get #first(){return()=>1}read(){return this.#first.name}}const c=new C();expect(c.read()).toBe('first')",
        "class C{#first(){return 1}read(){const key=unknown();return this.#first[key]}}const c=new C();expect(c.read()).toBe('first')",
    ] {
        let source=format!("import {{test,expect}} from 'vitest';test('first',()=>{{{body}}});test('second',()=>{{{body}}});");
        assert!(class_outcome_findings(&source,2,2).is_empty(),"{source}");
    }
    let mixed="import {test,expect} from 'vitest';test('one',()=>{class C{#first(){return 1}read(){return this.#first()}}const c=new C();expect(c.read()).toBe(1)});test('two',()=>{class C{#second(){return 1}read(){return this.#second()}}const c=new C();expect(c.read()).toBe(1)});test('unknown',()=>{class C{#first(){return 1}read(){return this.#first.toString()}}const c=new C();expect(c.read()).toBe('source')});";
    let findings = class_outcome_findings(mixed, 3, 1);
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}

/// Exact getter-returned callable uncertainty survives local aliases and stays visible.
#[test]
fn owned_class_private_getter_alias_uncertainty() {
    let source="import {test,expect} from 'vitest';\ntest('first',()=>{class C{get #fn(){return function first(){return 1}}read(){const alias=this.#fn;return alias.name}}const c=new C();expect(c.read()).toBe('first')});\ntest('second',()=>{class C{get #fn(){return function second(){return 1}}read(){const alias=this.#fn;return alias.name}}const c=new C();expect(c.read()).toBe('first')});\n";
    assert!(class_outcome_findings(source, 2, 2).is_empty());
    let computed = source.replace(
        "return alias.name",
        "const nested=alias;return nested['name']",
    );
    assert!(class_outcome_findings(&computed, 2, 2).is_empty());
}

/// Declared helpers retain equivalence, capture state, and observed callable names.
#[test]
fn owned_class_declared_helpers_and_callable_names() {
    use crate::model::Rule;
    let observed = "import {test,expect} from 'vitest';\ntest('first',()=>{function read(){return 'ready'}class C{field=read()}const c=new C();expect(c.field).toBe('ready')});\ntest('second',()=>{function read(){return 'ready'}class C{field=read()}const c=new C();expect(c.field).toBe('ready')});\n";
    assert_eq!(
        complete_pair_findings(observed)
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
    for (first, second, duplicate) in [
        ("const input = 'ready'; function read(){return input} class C{field=read()} const c=new C(); expect(c.field).toBe('ready');",
         "const renamed = 'ready'; function load(){return renamed} class C{field=load()} const c=new C(); expect(c.field).toBe('ready');", true),
        ("function read(){return first()} class C{field=read()} const c=new C(); expect(c.field).toBe('ready');",
         "function read(){return second()} class C{field=read()} const c=new C(); expect(c.field).toBe('ready');", false),
        ("const input = 'ready'; function read(){return input} class C{field=read()} const c=new C(); expect(c.field).toBe('ready');",
         "const input = 99; function read(){return input} class C{field=read()} const c=new C(); expect(c.field).toBe('ready');", false),
        ("const first=()=>1;class C{field=first.name}const c=new C();expect(c.field).toBe('first')",
         "const second=()=>1;class C{field=second.name}const c=new C();expect(c.field).toBe('first')", false),
        ("function first(){return 1} const alias=first; class C{field=alias['name']} const c=new C();expect(c.field).toBe('first')",
         "function second(){return 1} const alias=second; class C{field=alias['name']} const c=new C();expect(c.field).toBe('first')", false),
        ("const read=function stable(){return 1}; class C{field=read.name} const c=new C();expect(c.field).toBe('stable')",
         "const renamed=function stable(){return 1}; class C{field=renamed.name} const c=new C();expect(c.field).toBe('stable')", true),
    ] {
        let source = format!("import {{test,expect}} from 'vitest';test('first',()=>{{{first}}});test('second',()=>{{{second}}});");
        let findings=complete_pair_findings(&source);
        if duplicate { assert_eq!(findings.iter().filter(|finding| finding.rule == Rule::DuplicateTest).count(),1,"{source}: {findings:?}"); }
        else { assert!(!findings.iter().any(|finding|matches!(finding.rule,Rule::DuplicateTest|Rule::NearDuplicateTest)),"{source}: {findings:?}"); }
    }
    for body in [
        "const read=()=>1; class C{field=read.toString()} new C(); expect(1).toBe(1)",
        "function read(){return 1} const alias=read; Object.defineProperty(alias,'name',{value:'changed'}); class C{field=read.name} new C(); expect(1).toBe(1)",
        "const read=()=>1; class C{field=read.name} Object.defineProperty(read,'name',{value:'changed'}); new C(); expect(1).toBe(1)",
        "const read=()=>1; class C{field=read['toString']()} new C(); expect(1).toBe(1)",
        "const read=()=>1;const key=unknown();class C{field=read[key]} new C();expect(1).toBe(1)",
        "const read=()=>1;let key='name';class C{field=read[key]}key='toString';new C();expect(1).toBe(1)",
        "const input=[]; function read(){return input} class C{field=read()} new C(); expect(1).toBe(1)",
    ] {
        let source=format!("import {{test,expect}} from 'vitest';test('first',()=>{{{body}}});test('second',()=>{{{body}}});");
        assert!(class_outcome_findings(&source,2,2).is_empty(),"{source}");
    }
}

/// Advertised field/accessor, definition effects and observed static names stay comparable.
#[test]
fn owned_class_advertised_member_branches() {
    use crate::model::Rule;
    for body in [
        "class C{field:number; accessor value:number=1; static accessor shared:number=2} const c=new C(); expect(c.value+C.shared).toBe(3)",
        "class C{static field:number; static ['ready']=1; value:string='ready'} const c=new C(); expect(c.value).toBe('ready')",
        "const key='ready'; class C{static [key]=1; accessor [key]=2} const c=new C(); expect(c.ready+C.ready).toBe(3)",
        "const Factory=class {value=1}; const Alias=Factory; class C{field=Alias.name} const c=new C(); expect(c.field).toBe('Factory')",
        "class Base{value=1} class C extends Base{get ready(){return this.value}} const c=new C(); expect(c.ready).toBe(1)",
        "class C{static name='owned'; value=1} const c=new C(); expect(C.name).toBe('owned')",
    ] {
        let source=format!("import {{test,expect}} from 'vitest';test('first',()=>{{{body}}});test('second',()=>{{{body}}});");
        assert_eq!(complete_pair_findings(&source).iter().filter(|finding|finding.rule==Rule::DuplicateTest).count(),1,"{source}");
    }
    for (first, second) in [
        (
            "class C{accessor value=first()} const c=new C(); expect(c.value).toBe(1)",
            "class C{accessor value=second()} const c=new C(); expect(c.value).toBe(1)",
        ),
        (
            "class C{static accessor value=first()} new C(); expect(C.value).toBe(1)",
            "class C{accessor value=first()} const c=new C(); expect(c.value).toBe(1)",
        ),
        (
            "class C{static name='first'} new C(); expect(C.name).toBe('first')",
            "class C{static name='second'} new C(); expect(C.name).toBe('first')",
        ),
    ] {
        let source=format!("import {{test,expect}} from 'vitest';test('first',()=>{{{first}}});test('second',()=>{{{second}}});");
        let findings = complete_pair_findings(&source);
        assert!(
            !findings.iter().any(|finding| matches!(
                finding.rule,
                Rule::DuplicateTest | Rule::NearDuplicateTest
            )),
            "{source}: {findings:?}"
        );
    }
}

/// A stable owned closure can supply a modeled class value.
#[test]
fn owned_class_review_captured_closure() {
    let source = "import {test,expect} from 'vitest'; test('a',()=>{const helper=()=>1; class C{value=helper()} new C(); expect(1).toBe(1)}); test('b',()=>{const helper=()=>1; class C{value=helper()} new C(); expect(1).toBe(1)});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// A stable Vitest mock is an owned initialization effect, rather than an unknown capture.
#[test]
fn owned_class_review_captured_mock() {
    let source = "import {test,expect,vi} from 'vitest'; test('a',()=>{const spy=vi.fn(); class C{constructor(){spy()}} new C(); expect(spy).toHaveBeenCalledOnce()}); test('b',()=>{const spy=vi.fn(); class C{constructor(){spy()}} new C(); expect(spy).toHaveBeenCalledOnce()});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// TypeScript parameter properties retain their field and constructor-parameter relationship.
#[test]
fn owned_class_review_parameter_properties() {
    let source = "import {test,expect} from 'vitest'; test('a',()=>{class C{constructor(private x:number){}} const c=new C(1); expect(c).toBeDefined()}); test('b',()=>{class C{constructor(private x:number){}} const c=new C(1); expect(c).toBeDefined()});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// Proven owned construction effects remain comparable through a later instance method read.
#[test]
fn owned_class_review_mutable_capture_instance_read() {
    let source = "import {test,expect} from 'vitest'; test('a',()=>{const trace=[]; class C{constructor(){trace.push(1)} read(){return trace}} const c=new C(); expect(c.read()).toEqual([1])}); test('b',()=>{const trace=[]; class C{constructor(){trace.push(1)} read(){return trace}} const c=new C(); expect(c.read()).toEqual([1])});";
    assert_eq!(complete_pair_findings(source).len(), 1);
}

/// Independent runtime-backed holdouts retain owned state and effect ordering.
#[test]
fn owned_class_review_independent_state_history() {
    use crate::model::Rule;
    let source = r#"import { test, expect } from 'vitest';
test('first',()=>{const trace=[];class Box{constructor(){trace.push(1)}read(){return trace}}const box=new Box();expect(box.read()).toEqual([1])});
test('second',()=>{const log=[];class Container{constructor(){log.push(1)}read(){return log}}const item=new Container();expect(item.read()).toEqual([1])});
"#;
    let findings = complete_pair_findings(source);
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1,
        "{findings:?}"
    );
    let source = r#"import { test, expect } from 'vitest';
test('first',()=>{const trace=[];class C{constructor(){trace.push(1)}read(){return trace}}const c=new C();expect(c.read()).toEqual([1])});
test('second',()=>{const trace=['seed'];class C{constructor(){trace.push(1)}read(){return trace}}const c=new C();expect(c.read()).toEqual([1])});
"#;
    let findings = complete_pair_findings(source);
    assert_class_distinct(&findings, source);
    let source = r#"import { test, expect } from 'vitest';
test('first',()=>{const trace=[];class C{static field=trace.push('field');static{trace.push('block')}read(){return trace}}const c=new C();expect(c.read()).toEqual(['field','block'])});
test('second',()=>{const trace=[];class C{static{trace.push('block')}static field=trace.push('field');read(){return trace}}const c=new C();expect(c.read()).toEqual(['field','block'])});
"#;
    let findings = complete_pair_findings(source);
    assert_class_distinct(&findings, source);
    let source = r#"import { test, expect } from 'vitest';
test('first',()=>{let value='ready';class C{field=value;read(){return this.field}}const c=new C();expect(c.read()).toBe('ready')});
test('second',()=>{let value='ready';class C{field=value;read(){return this.field}}value={changed:true};const c=new C();expect(c.read()).toBe('ready')});
"#;
    let findings = complete_pair_findings(source);
    assert_class_distinct(&findings, source);
    let repeated = "import {test,expect} from 'vitest'; test('a',()=>{const trace=[]; class C{constructor(){trace.push(1)}} new C(); new C(); expect(trace).toEqual([1,1])}); test('b',()=>{const trace=[]; class C{constructor(){trace.push(1)}} new C(); new C(); expect(trace).toEqual([1,1])});";
    assert_eq!(
        complete_pair_findings(repeated)
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}

/// A known constructor name is an observed value, preserving literal consolidation.
#[test]
fn owned_class_review_known_constructor_names() {
    use crate::model::Rule;
    let source = r#"import { test, expect } from 'vitest';
test('first',()=>{class First{field=1}const value=new First();expect(value.constructor.name).toBe('First')});
test('second',()=>{class Second{field=1}const value=new Second();expect(value.constructor.name).toBe('First')});
"#;
    let findings = complete_pair_findings(source);
    assert!(!findings
        .iter()
        .any(|finding| matches!(finding.rule, Rule::DuplicateTest | Rule::NearDuplicateTest)));
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
            .count(),
        1
    );
    let aliases = "import {test,expect} from 'vitest'; test('a',()=>{class C{} const K=C; expect(K.name).toBe('C')}); test('b',()=>{class C{} const Alias=C; expect(Alias.name).toBe('C')});";
    assert_eq!(
        complete_pair_findings(aliases)
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}

/// Owned class uncertainty reaches helpers and setup dependencies without silencing safe bodies.
#[test]
fn owned_class_dependency_uncertainty_reaches_all_consumers() {
    use crate::model::Rule;
    let helper = "export function make(flag) { let input=1; if(flag) input=2; class C { field=input } return new C() }";
    let safe = "test('safe first',()=>{class C{field=1} new C(); expect(1).toBe(1)}); test('safe second',()=>{class C{field=1} new C(); expect(1).toBe(1)});";
    let source = format!("import {{test,expect}} from 'vitest'; import {{make}} from './helper.js'; test('first',()=>{{const value=make(true);expect(value.field).toBe(2)}}); test('second',()=>{{const value=make(true);expect(value.field).toBe(2)}});{safe}");
    let findings = class_outcome_findings_with_files(&source, 4, 2, &[("helper.js", helper)]);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    let hook = format!("import {{test,expect,describe,beforeEach}} from 'vitest'; describe('uncertain',()=>{{beforeEach(()=>{{let input=1;if(flag)input=2;class C{{field=input}}new C()}});test('a',()=>expect(1).toBe(1));test('b',()=>expect(1).toBe(1))}});{safe}");
    assert_eq!(class_outcome_findings(&hook, 5, 2).len(), 1);
    let fixture = format!("import {{test,expect}} from 'vitest'; const custom=test.extend({{value: async ({{}},use)=>{{let input=1;if(flag)input=2;class C{{field=input}}await use(new C())}}}});custom('first',({{value}})=>expect(value).toBeDefined());custom('second',({{value}})=>expect(value).toBeDefined());{safe}");
    assert_eq!(class_outcome_findings(&fixture, 4, 2).len(), 1);
    let mock = "import {test,expect,vi} from 'vitest';vi.mock('./module',()=>{let input=1;if(flag)input=2;class C{field=input}return new C()});test('first',()=>expect(1).toBe(1));test('second',()=>expect(1).toBe(1));";
    assert!(class_outcome_findings(mock, 2, 2).is_empty());
}

/// One ordered prefix retains interleaving between owned construction and direct mutations.
#[test]
fn owned_class_history_preserves_interleaved_operations() {
    let source = "import {test,expect} from 'vitest';test('first',()=>{const trace=[];class C{constructor(value){trace.push(value)}}new C('a');trace.push('external');new C('b');expect(trace).toEqual(['a','external','b'])});test('second',()=>{const trace=[];class C{constructor(value){trace.push(value)}}trace.push('external');new C('a');new C('b');expect(trace).toEqual(['a','external','b'])});";
    let findings = complete_pair_findings(source);
    assert_class_distinct(&findings, source);
}

/// Enabled subsumption retains strict constructor supersets and rejects changed class effects.
#[test]
fn owned_classes_participate_in_enabled_subsumption() {
    use crate::model::Rule;
    let settings = r#"{"rules":{"subsumed-test":"warning"}}"#;
    for (first, second, expected) in [
        (
            "new Store(1); expect(trace).toEqual([1]);",
            "new Store(1); tick(); expect(trace).toEqual([1]);",
            1,
        ),
        (
            "class C{constructor(){trace.push(1)}} new C(); expect(trace).toEqual([1]);",
            "class C{constructor(){trace.push(1)}} new C(); tick(); expect(trace).toEqual([1]);",
            1,
        ),
        (
            "class C{constructor(){trace.push(1)}} new C(); expect(trace).toEqual([1]);",
            "class C{constructor(){trace.push(2)}} new C(); tick(); expect(trace).toEqual([1]);",
            0,
        ),
        (
            "new Date(0); expect(trace).toEqual([1]);",
            "new Map([]); tick(); expect(trace).toEqual([1]);",
            0,
        ),
        (
            "new First(); expect(trace).toEqual([1]);",
            "new Second(); tick(); expect(trace).toEqual([1]);",
            0,
        ),
        (
            "new Store(first(),second()); expect(trace).toEqual([1]);",
            "new Store(second(),first()); tick(); expect(trace).toEqual([1]);",
            0,
        ),
    ] {
        let source = format!("import {{test,expect}} from 'vitest'; import {{First,Second}} from './constructors.js'; const trace=[];test('small',()=>{{{first}}});test('large',()=>{{{second}}});");
        let findings = class_outcome_findings_with_files(
            &source,
            2,
            0,
            &[
                ("settings.json", settings),
                (
                    "constructors.js",
                    "export class First{} export class Second{}",
                ),
            ],
        );
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == Rule::SubsumedTest)
                .count(),
            expected,
            "{source}: {findings:?}"
        );
    }
}

/// Constructor traversal retains nested effects in source order and deferred scope.
#[test]
fn constructor_nested_events_keep_scope_and_argument_order() {
    let events = behavior_of("new Store(first(), second());");
    assert!(events[0].starts_with("new:"), "{events:?}");
    let first = events
        .iter()
        .position(|event| event.starts_with("call:global:first"))
        .unwrap();
    let second = events
        .iter()
        .position(|event| event.starts_with("call:global:second"))
        .unwrap();
    assert!(first < second, "{events:?}");
    let nested = behavior_of("function build() { new Store(first()); } build();");
    assert!(
        nested
            .iter()
            .any(|event| event.starts_with("deferred-new:")),
        "{nested:?}"
    );
    assert!(
        !nested.iter().any(|event| event.starts_with("new:")),
        "{nested:?}"
    );
}
/// Check final rule outcomes with explicit extraction and completeness controls.
fn constructor_rules(first: &str, second: &str) -> Vec<crate::model::Rule> {
    use crate::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join(".vitest-dedup.json");
    std::fs::write(&settings, r#"{"rules":{"subsumed-test":"warning"}}"#).unwrap();
    let config = crate::config::Config::load(directory.path(), Default::default()).unwrap();
    let source = format!("import {{test,expect}} from 'vitest';import {{Store,Cart}} from './store.js';class Local{{constructor(){{trace.push(2)}}}}test('first',()=>{first});test('second',()=>{second});");
    let file = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::ForcedEsm,
    );
    let extracted = crate::frontend::extract(&source, &file, crate::frontend::Globals::Disabled);
    assert_eq!(extracted.tests.len(), 2);
    assert!(!extracted.incomplete);
    let run = crate::analysis::analyze(extracted.tests, &config, 1, Default::default()).unwrap();
    assert_eq!(run.unresolved_tests, 0);
    assert!(!run.incomplete());
    run.findings.into_iter().map(|f| f.rule).collect()
}

/// Constructor evidence rejects changed work and retains equivalent and strict-superset cases.
#[test]
fn external_constructor_outcomes() {
    use crate::model::Rule::*;
    for (first, second) in [
        ("new Store(loadA())", "new Cart(loadA())"),
        ("new Store(loadA())", "new Store(loadB())"),
        ("new Store<string>(1)", "new Store<number>(1)"),
        ("new Store(...a)", "new Store(...b)"),
        ("new Store(1)", "Store(1)"),
    ] {
        let rules = constructor_rules(
            &format!("{{{first};tick();expect(trace).toEqual([1])}}"),
            &format!("{{{second};tick();expect(trace).toEqual([1])}}"),
        );
        assert!(
            !rules.contains(&NearDuplicateTest) && !rules.contains(&SubsumedTest),
            "{first}: {rules:?}"
        );
    }
    for (first, second, rule) in [
        ("new Store(1);", "new Store(1);", DuplicateTest),
        ("new Store(1);", "void 0;new Store(1);", NearDuplicateTest),
        ("new Store(1);", "new Store(2);", ParameterizationCandidate),
        ("new Store(1);", "new Store(1);tick();", SubsumedTest),
        (
            "class Local{constructor(){trace.push(2)}}new Store(1);new Local();",
            "new Store(1);",
            SubsumedTest,
        ),
        (
            "class C{};new C();tick();",
            "class C{};void 0;new C();tick();",
            NearDuplicateTest,
        ),
    ] {
        assert!(constructor_rules(
            &format!("{{{first}expect(trace).toEqual([1])}}"),
            &format!("{{{second}expect(trace).toEqual([1])}}")
        )
        .contains(&rule));
    }
}

/// Constructor events retain local and dynamic evidence in source order and deferred scope.
#[test]
fn external_constructor_scope_and_order() {
    for body in [
        "class C{};new C();",
        "const C=Store;new C();",
        "new (factory())();",
        "new ns[pick()]();",
        "new Store(new Local());class Local{}",
        "class Local{};const x=new Local();new Store(x);",
        "new Store();expect(new Local()).toBeDefined();class Local{}",
    ] {
        assert!(
            behavior_of(body)
                .iter()
                .any(|event| event.starts_with("new:")),
            "{body}"
        );
    }
    let rules = constructor_rules(
        "load().then(()=>{new Store(1);new Local();tick();expect(trace).toEqual([1])})",
        "load().then(()=>{new Store(1);tick();expect(trace).toEqual([1])})",
    );
    assert!(
        rules.contains(&crate::model::Rule::SubsumedTest),
        "{rules:?}"
    );
    assert!(!constructor_rules(
        "load().then(()=>{new Store(1);new Local();tick();expect(trace).toEqual([1])})",
        "load().then(()=>{new Cart(1);tick();expect(trace).toEqual([1])})",
    )
    .contains(&crate::model::Rule::SubsumedTest));
    let events = behavior_of("new Store(first(), ...second());");
    assert!(events[0].starts_with("new:"));
    assert!(
        events[1].contains("first") && events[2].contains("second"),
        "{events:?}"
    );
    assert!(behavior_of("function f(){new Store()}f();")
        .iter()
        .any(|event| event.starts_with("deferred-new:")));
}

/// Newly visited constructor positions distinguish subjects without silencing valid matches.
#[test]
fn constructor_traversal_outcomes() {
    use crate::model::Rule::*;
    for (first, second) in [
        ("for(new Store();false;){}", "for(new Cart();false;){}"),
        (
            "for(const value=new Store();false;){}",
            "for(const value=new Cart();false;){}",
        ),
        ("`${new Store()}`;", "`${new Cart()}`;"),
        (
            "for(const value=seedA();false;){}",
            "for(const value=seedB();false;){}",
        ),
        ("`${seedA()}`;", "`${seedB()}`;"),
        ("`${`${new Store()}`}`;", "`${`${new Cart()}`}`;"),
    ] {
        let rules = constructor_rules(
            &format!("{{{first}tick();expect(trace).toEqual([1])}}"),
            &format!("{{{second}tick();expect(trace).toEqual([1])}}"),
        );
        assert!(rules.is_empty(), "{first}: {rules:?}");
    }
    for (first, second, rule) in [
        (
            "for(new Store(1);false;){}",
            "for(new Store(1);false;){}",
            DuplicateTest,
        ),
        (
            "for(const v=new Store(1);false;){}",
            "void 0;for(const v=new Store(1);false;){}",
            NearDuplicateTest,
        ),
        (
            "`${new Store(1)}`;",
            "`${new Store(2)}`;",
            ParameterizationCandidate,
        ),
        (
            "`${new Store(1)}`;",
            "`${new Store(1)}`;extra();",
            SubsumedTest,
        ),
        ("`${seed()}`;", "`${seed()}`;extra();", SubsumedTest),
        (
            "class C{};for(new C();false;){}",
            "class C{};void 0;for(new C();false;){}",
            NearDuplicateTest,
        ),
    ] {
        assert!(
            constructor_rules(
                &format!("{{{first}tick();expect(trace).toEqual([1])}}"),
                &format!("{{{second}tick();expect(trace).toEqual([1])}}")
            )
            .contains(&rule),
            "{first}: expected {rule:?}"
        );
    }
}

/// Erased callee wrappers preserve constructor identity, effects and valid findings.
#[test]
fn constructor_wrapper_outcomes() {
    use crate::model::Rule::*;
    for wrapper in [
        "({})",
        "({} as any)",
        "({} satisfies any)",
        "{}!",
        "(<any>{})",
        "(({} as any)!)",
    ] {
        let store = wrapper.replace("{}", "Store");
        let cart = wrapper.replace("{}", "Cart");
        let first = format!("{{new {store}();tick();expect(trace).toEqual([1])}}");
        let second = format!("{{new {cart}();tick();expect(trace).toEqual([1])}}");
        assert!(constructor_rules(&first, &second).is_empty(), "{wrapper}");
        for (first, second, rule) in [
            (
                format!("new {store}(1);"),
                format!("new {store}(1);"),
                DuplicateTest,
            ),
            (
                "new Store(1);".into(),
                format!("void 0;new {store}(1);"),
                NearDuplicateTest,
            ),
            (
                format!("new {store}(1);"),
                format!("new {store}(2);"),
                ParameterizationCandidate,
            ),
            (
                format!("new {store}(1);"),
                format!("new {store}(1);extra();"),
                SubsumedTest,
            ),
        ] {
            assert!(
                constructor_rules(
                    &format!("{{{first}tick();expect(trace).toEqual([1])}}"),
                    &format!("{{{second}tick();expect(trace).toEqual([1])}}"),
                )
                .contains(&rule),
                "{wrapper}: expected {rule:?}"
            );
        }
    }
    for (first, second) in [
        ("new (Store as any)(loadA())", "new (Store as any)(loadB())"),
        (
            "new (Store as any)<string>()",
            "new (Store as any)<number>()",
        ),
        (
            "new (Store as any)(first(),second())",
            "new (Store as any)(second(),first())",
        ),
        (
            "new ((Store as any).Builder)()",
            "new ((Store as any).Other)()",
        ),
        ("new (Store as any)()", "(Store as any)()"),
    ] {
        assert!(
            constructor_rules(
                &format!("{{{first};tick();expect(trace).toEqual([1])}}"),
                &format!("{{{second};tick();expect(trace).toEqual([1])}}"),
            )
            .is_empty(),
            "{first}"
        );
    }
    assert!(constructor_rules(
        "{new Store.Builder();tick();expect(trace).toEqual([1])}",
        "{void 0;new ((Store satisfies any).Builder)();tick();expect(trace).toEqual([1])}",
    )
    .contains(&NearDuplicateTest));
}

/// Erased wrappers preserve local and dynamic constructor evidence and depth degradation.
#[test]
fn constructor_wrapper_scope() {
    for (wrapped, plain) in [
        ("class C{};new (C as any)();", "class C{};new C();"),
        (
            "const C=Store;new (C satisfies any)();",
            "const C=Store;new C();",
        ),
        (
            "const Store=class{};new Store!();",
            "const Store=class{};new Store();",
        ),
        ("new ((factory()) as any)();", "new (factory())();"),
        ("new (ns[pick()] as any)();", "new (ns[pick()])();"),
        (
            "new Store();expect(new (Local as any)()).toBeDefined();class Local{}",
            "new Store();expect(new Local()).toBeDefined();class Local{}",
        ),
        (
            "function f(){class C{};new (C as any)();}new Store();",
            "function f(){class C{};new C();}new Store();",
        ),
    ] {
        let events = behavior_of(&format!("{wrapped}tick();"));
        assert!(!events.is_empty(), "{wrapped}");
        assert_eq!(events, behavior_of(&format!("{plain}tick();")), "{wrapped}");
    }
    assert_ne!(
        behavior_of("new (Store<string>)();"),
        behavior_of("new Store();")
    );
    assert!(constructor_rules(
        "{new (<any>Local)();tick();expect(trace).toEqual([1])}",
        "{new Store();tick();expect(trace).toEqual([1])}",
    )
    .is_empty());
    let events = behavior_of("new ((Store as any) satisfies any)!(first(),...second());");
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(events[0].starts_with("new:"), "{events:?}");
    assert!(
        events[1].contains("first") && events[2].contains("second"),
        "{events:?}"
    );
    assert_eq!(
        behavior_of("function f(){new (Store as any)()}"),
        behavior_of("function f(){new Store()}"),
    );
    let callee = format!(
        "{}Store{}",
        "(".repeat(MAX_DEPTH as usize + 1),
        ")".repeat(MAX_DEPTH as usize + 1)
    );
    let events = behavior_of(&format!("new {callee}();tick();"));
    assert!(
        events.iter().any(|event| event.contains("<max-depth:")),
        "{events:?}"
    );
    let source = format!("import {{test,expect}} from 'vitest';test('first',()=>{{new {callee}();tick()}});test('second',()=>{{new {callee}();tick()}});");
    let file = crate::discovery::SourceFile::without_project_context(
        std::path::PathBuf::from("deep-constructor.test.ts"),
        crate::discovery::SourceLanguage::TypeScript,
        crate::discovery::ModuleFlavor::ForcedEsm,
    );
    let extracted = crate::frontend::extract(&source, &file, crate::frontend::Globals::Disabled);
    assert_eq!(extracted.tests.len(), 0);
    assert!(extracted.incomplete);
    assert!(!extracted.diagnostics.is_empty());
}

/// Initializers/interpolations retain source order, deferred scope and single-use inlining.
#[test]
fn constructor_traversal_order_and_deferred() {
    let events =
        behavior_of("for(const a=first(),b=new Store(second());condition();update()){body()}");
    assert!(events[0].starts_with("call:global:first"), "{events:?}");
    assert!(
        events[1].starts_with("new:") && events[2].starts_with("call:global:second"),
        "{events:?}"
    );
    assert_eq!(events[3], "loop");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.starts_with("new:"))
            .count(),
        1
    );
    let events = behavior_of("`${first()}${new Store(second())}${third()}`;");
    assert_eq!(events.len(), 4, "{events:?}");
    for (event, prefix) in events.iter().zip([
        "call:global:first",
        "new:",
        "call:global:second",
        "call:global:third",
    ]) {
        assert!(event.starts_with(prefix), "{events:?}");
    }
    let events = behavior_of("function build(){for(new Store();false;){} `${new Store()}`;}");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.starts_with("deferred-new:"))
            .count(),
        2
    );
    assert_eq!(
        behavior_of("const value=`${new Store(1)}`;consume(value);"),
        behavior_of("consume(`${new Store(1)}`);")
    );
}
