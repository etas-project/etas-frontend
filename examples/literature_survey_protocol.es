module examples.literature_survey_protocol;

type Citation = { title: string, url: string, year: u32 };
type SurveyRequest = { topic: string, max_papers: u32 };
type Claim = { text: string, citation: Citation };
type Survey = { claims: List[Claim], gaps: List[string] };
type Critique = { accepted: bool, notes: string };

protocol SurveyReview {
  Extractor -> Synthesizer: List[Claim];
  Synthesizer -> Critic: Survey;
  Critic -> Synthesizer: Critique;
}

@model(model = "gpt-5.5")
agent Extractor(input: SurveyRequest) -> Message[List[Claim]] {
  return Prompt.new()
    .system(Trusted("Extract evidence-carrying claims."))
    .data(input);
}

@model(model = "gpt-5.5-thinking")
agent Synthesizer(input: Message[List[Claim]]) -> Message[Survey] {
  return Prompt.new()
    .system(Trusted("Synthesize claims into a survey."))
    .data(input);
}

@model(model = "gpt-5.5-thinking")
agent Critic(input: Message[Survey]) -> Message[Critique] {
  return Prompt.new()
    .system(Trusted("Check unsupported claims and missing citations."))
    .data(input);
}

flow WriteSurvey(req: SurveyRequest) -> Message[Survey] ~ SurveyReview
{
  let claims = req ~> Extractor;
  var survey = claims ~> Synthesizer;
  var critique = survey ~> Critic;

  while !critique.body.accepted limit Iterations(2) {
    survey = claims ~> Synthesizer;
    critique = survey ~> Critic;
  }

  return survey;
}
