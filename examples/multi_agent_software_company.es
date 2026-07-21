module examples.multi_agent_software_company;

alias Path = string;
type Brief = { title: string, goal: string };
type ProductSpec = { requirements: List[string], risks: List[string] };
type Architecture = { components: List[string], notes: string };
type ImplementationPlan = { tasks: List[string], files: List[Path] };
type ReviewReport = { accepted: bool, comments: List[string] };
type ProjectResult = { spec: ProductSpec, architecture: Architecture, plan: ImplementationPlan };

memory CompanyMemory {
  Specs: Map[string, ProductSpec];
  Architectures: Map[string, Architecture];
  Plans: Map[string, ImplementationPlan];
}

@model(model = "gpt-5.5")
agent ProductManager(input: Brief) -> ProductSpec {
  return Prompt.new()
    .system(Trusted("Turn a brief into product requirements."))
    .data(input);
}

@model(model = "gpt-5.5-thinking")
agent Architect(input: ProductSpec) -> Architecture {
  return Prompt.new()
    .system(Trusted("Design a pragmatic software architecture."))
    .data(input);
}

@model(model = "gpt-5.5-coder")
agent Engineer(input: { spec: ProductSpec, architecture: Architecture }) -> ImplementationPlan {
  return Prompt.new()
    .system(Trusted("Create an implementation plan."))
    .data(input);
}

@model(model = "gpt-5.5-thinking")
agent Reviewer(input: ImplementationPlan) -> ReviewReport {
  return Prompt.new()
    .system(Trusted("Review plan risk and completeness."))
    .data(input);
}

flow BuildProject(brief: Brief) -> ProjectResult {
  let spec = brief ~> ProductManager;
  let architecture = spec ~> Architect;
  var plan = Engineer.run({ spec: spec, architecture: architecture });

  var review = Reviewer.run(plan);
  while !review.accepted limit Iterations(3) {
    plan = Engineer.run({ spec: spec, architecture: architecture });
    review = Reviewer.run(plan);
  }

  CompanyMemory.Specs.put(brief.title, spec);
  CompanyMemory.Architectures.put(brief.title, architecture);
  CompanyMemory.Plans.put(brief.title, plan);

  return ProjectResult { spec, architecture, plan };
}
