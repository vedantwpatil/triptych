# Triptych

Combination of multiple productivity apps into one specialized for my personal command line use case

### Future Plans

- [ ] Create a schedule in which everything is either 30 minute or 15 minute blocks. This will be loaded in from a json file using serde
- [ ] You can quickly add tasks to the next block (add "prepare for cs 281 (class name) final" and this would be added to the next academic block)
  - [ ] Should also be able to add tasks for different classes to be able to list homework
- [ ] You can also easily add daily tasks that show up on a all day calendar

#### Planning/Sounding out Idea

I am currently thinking about how I want this UI to look like. What is the thing that I would want the user to be created with on startup, where should I start and what the gameplay is.

First we should be looking at our MVP and what that looks like, this would be just creating something which is able to store tasks in a database and being able to reference them later on. The simple to do list is the first thing, after that we can then add the calendar/schedule schemas that we can easily add tasks to based off of what category that they fall into (academic/school work, corporate work, free time tasks, such and such) this allows us to quickly add these new tasks to their respective fields.

This would require some standard alp to understand which tasks belong to which, this should be something that is pretty simple to borrow from an existing project or we could overhaul the ROBERTA framework with the extensively checked and cleaned training data however that is more for the web scraping project I have planned
