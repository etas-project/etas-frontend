module examples.multi_agent_software_company;

import std.agent.prompt.Prompt;
import std.memory.{Store, region};
import std.task.{Scheduler, run};

type Brief = { title: string, goal: string };
type ProductSpec = { requirements: List<string>, risks: List<string> };
type Architecture = { components: List<string>, notes: string };
type ImplementationPlan = { tasks: List<string>, files: List<string> };
type ReviewReport = { accepted: bool, comments: List<string> };
type ProjectResult = { requirements: ProductSpec, architecture: Architecture, plan: ImplementationPlan };

type CompanySchema = {
    Specs: Store<string, ProductSpec>,
    Architectures: Store<string, Architecture>,
    Plans: Store<string, ImplementationPlan>,
};

let CompanyMemory = region<CompanySchema>(stable_id = "software_company", store = "software-company");

@model("gpt-5.5")
agent ProductManager = {};

impl ProductManager {
    public flow run(self, brief: Brief) -> ProductSpec {
        return perform infer(Prompt.new()
            .system(Trusted("Turn a brief into product requirements."))
            .data(brief.title).data(brief.goal));
    }
}

@model("gpt-5.5-thinking")
agent Architect = {};

impl Architect {
    public flow run(self, product: ProductSpec) -> Architecture {
        return perform infer(Prompt.new()
            .system(Trusted("Design a pragmatic software architecture."))
            .data(product.requirements).data(product.risks));
    }
}

@model("gpt-5.5-coder")
agent Engineer = {};

impl Engineer {
    public flow plan(self, product: ProductSpec, architecture: Architecture, comments: List<string>) -> ImplementationPlan {
        return perform infer(Prompt.new()
            .system(Trusted("Create an implementation plan and address the review comments."))
            .data(product.requirements).data(product.risks)
            .data(architecture.components).data(architecture.notes).data(comments));
    }
}

@model("gpt-5.5-thinking")
agent Reviewer = {};

impl Reviewer {
    public flow run(self, plan: ImplementationPlan) -> ReviewReport {
        return perform infer(Prompt.new()
            .system(Trusted("Review plan risk and completeness."))
            .data(plan.tasks).data(plan.files));
    }
}

public flow BuildProject(brief: Brief) -> ProjectResult ![Memory, Error<std.memory.StorageError>] {
    return run((scheduler: Scheduler) => {
        let product = ProductManager {}.run(brief);
        let architecture = Architect {}.run(product);
        let engineer = Engineer {};
        let reviewer = Reviewer {};
        var plan = engineer.plan(product, architecture, []);
        var review = reviewer.run(plan);
        while !review.accepted {
            plan = engineer.plan(product, architecture, review.comments);
            review = reviewer.run(plan);
        }
        CompanyMemory.Specs.put(brief.title, product);
        CompanyMemory.Architectures.put(brief.title, architecture);
        CompanyMemory.Plans.put(brief.title, plan);
        return ProjectResult { requirements = product, architecture = architecture, plan = plan };
    });
}

flow main(args: Array<string>) -> i32 {
    let result = BuildProject(Brief { title = "Search", goal = "Index project documents" });
    std.io.println(result.architecture.notes);
    return 0;
}
