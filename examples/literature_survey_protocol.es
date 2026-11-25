module examples.literature_survey_protocol;

import std.agent.message.Message;
import std.agent.prompt.Prompt;
import std.task.{Scheduler, run};

type Citation = { title: string, url: string, year: u32 };
type SurveyRequest = { topic: string, max_papers: u32 };
type Claim = { text: string, citation: Citation };
type Survey = { claims: List<Claim>, gaps: List<string> };
type Critique = { accepted: bool, notes: string };

@model("gpt-5.5")
agent Extractor = {};

impl Extractor {
    public flow run(self, request: SurveyRequest) -> Message<List<Claim>> {
        let claims: List<Claim> = perform infer(Prompt.new()
            .system(Trusted("Extract evidence-carrying claims up to the supplied paper count."))
            .data(request.topic).data(request.max_papers));
        return Message.new(claims);
    }
}

@model("gpt-5.5-thinking")
agent Synthesizer = {};

impl Synthesizer {
    public flow run(self, claims: Message<List<Claim>>, review: Option<Message<Critique>>) -> Message<Survey> {
        var prompt = Prompt.new()
            .system(Trusted("Synthesize the cited claims into a survey, addressing review notes."));
        prompt = match review {
            Some(critique) => prompt.data(critique.content.notes),
            None => prompt,
        };
        for claim in claims.content {
            prompt = prompt.data(claim.text).data(claim.citation.title)
                .data(claim.citation.url).data(claim.citation.year);
        }
        let survey: Survey = perform infer(prompt);
        return Message.new(survey);
    }
}

@model("gpt-5.5-thinking")
agent Critic = {};

impl Critic {
    public flow run(self, survey: Message<Survey>) -> Message<Critique> {
        var prompt = Prompt.new()
            .system(Trusted("Check for unsupported claims and missing citations."))
            .data(survey.content.gaps);
        for claim in survey.content.claims {
            prompt = prompt.data(claim.text).data(claim.citation.title)
                .data(claim.citation.url).data(claim.citation.year);
        }
        let critique: Critique = perform infer(prompt);
        return Message.new(critique);
    }
}

protocol SurveyReview {
    Extractor -> Synthesizer: List<Claim>;
    Synthesizer -> Critic: Survey;
    Critic -> Synthesizer: Critique;
}

public flow WriteSurvey(request: SurveyRequest) -> Message<Survey> ~ SurveyReview {
    let claims = Extractor {}.run(request);
    let synthesizer = Synthesizer {};
    let critic = Critic {};
    var survey = synthesizer.run(claims, None);
    var critique = critic.run(survey);
    while !critique.content.accepted {
        survey = synthesizer.run(claims, Some(critique));
        critique = critic.run(survey);
    }
    return survey;
}

flow main(args: Array<string>) -> i32 {
    let result = run((scheduler: Scheduler) => WriteSurvey(SurveyRequest {
        topic = "Evidence quality in systematic reviews", max_papers = 5,
    }));
    for gap in result.content.gaps { std.io.println(gap); }
    return 0;
}
